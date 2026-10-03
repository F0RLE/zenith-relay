import { useEffect, useRef, useState, type RefObject } from "react";
import type { TFunction } from "i18next";
import { Loader2, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountImportProgress, ConfirmAccountImportResponse, ImportSession, RelayMode } from "../../api/types";
import { AccountPlanBadge, Button, Dialog, SettingToggle } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { captureOperationResult } from "../../state/relayOperationModel";
import {
  beginAccountImportConfirmation,
  finishAccountImportConfirmation,
} from "../../state/relayPreferences";
import { MarkdownPreview } from "../../components/MarkdownPreview";
import { useProxyPool } from "./useProxyPool";

type ImportFailure = { itemId: string; code: string; label?: string; identity?: string };

function selectedImportItemIds(session?: ImportSession) {
  return session?.preview.rows
    .filter((row) => row.defaultSelected)
    .map((row) => row.itemId) ?? [];
}

export function ImportDialog({
  initialPaths,
  initialSession,
  modeOverride,
  defaultAddToPool = false,
  onImported,
  onClose,
}: {
  initialPaths?: string[];
  initialSession?: ImportSession;
  modeOverride?: RelayMode;
  defaultAddToPool?: boolean;
  onImported?: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const { mode: currentMode, runtime, perform, busy } = useRelayState();
  const mode = modeOverride ?? currentMode;
  const { pool: proxyPool } = useProxyPool(mode === "local");
  const [content, setContent] = useState("");
  const [session, setSession] = useState<ImportSession | null>(initialSession ?? null);
  const [ownedSessionId, setOwnedSessionId] = useState<string | null>(initialSession?.sessionId ?? null);
  const [selected, setSelected] = useState<string[]>(() => selectedImportItemIds(initialSession));
  const [commandFailed, setCommandFailed] = useState(false);
  const [completed, setCompleted] = useState<ImportFailure[] | null>(null);
  const [progress, setProgress] = useState<AccountImportProgress | null>(null);
  const [addToPool, setAddToPool] = useState(defaultAddToPool);
  const [assignProxy, setAssignProxy] = useState(false);
  const [fileLoading, setFileLoading] = useState(Boolean(initialPaths?.length));
  const activeSessionId = useRef<string | null>(initialSession?.sessionId ?? null);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const initialPreviewStarted = useRef(false);
  const mounted = useRef(true);
  const confirmInFlight = useRef(false);
  const closing = useRef(false);
  const canImportToPool = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("account_import_to_pool"));
  const importOperationBusy = busy?.startsWith("import-") ?? false;
  const acceptSession = (next: ImportSession) => {
    setSession(next);
    setOwnedSessionId(next.sessionId);
    activeSessionId.current = next.sessionId;
    setCommandFailed(false);
    setCompleted(null);
    setProgress(null);
    setSelected(selectedImportItemIds(next));
  };
  const cancel = async () => {
    if (importOperationBusy || confirmInFlight.current || closing.current) return;
    closing.current = true;
    const sessionId = session?.sessionId ?? ownedSessionId;
    try {
      if (mode === "local" && sessionId) await perform("import-cancel", () => relayCommands.cancelImport(sessionId), undefined, { backgroundRefresh: true });
    } finally {
      activeSessionId.current = null;
      if (mounted.current) onClose();
    }
  };
  const preview = async () => {
    if (mode === "local") {
      let startedSessionId: string | null = null;
      const captured = await captureOperationResult(
        (work) => perform("import-preview", work, undefined, { backgroundRefresh: true }),
        async () => {
          const started = await relayCommands.startImport(content);
          startedSessionId = started.sessionId;
          activeSessionId.current = started.sessionId;
          if (mounted.current) setOwnedSessionId(started.sessionId);
          return relayCommands.prepareImport(started.sessionId, false);
        },
      );
      if (!mounted.current) {
        if (startedSessionId) void relayCommands.cancelImport(startedSessionId).catch(() => undefined);
        return;
      }
      if (captured.ok && captured.value) acceptSession(captured.value);
      else {
        // `startImport` creates a temporary session before parsing.  If the
        // parser/metadata probe fails, release that session immediately so a
        // repeated preview cannot accumulate stale state or crash cleanup.
        if (startedSessionId) {
          if (activeSessionId.current === startedSessionId) activeSessionId.current = null;
          setOwnedSessionId((current) => current === startedSessionId ? null : current);
          void relayCommands.cancelImport(startedSessionId).catch(() => undefined);
        }
        setCommandFailed(true);
      }
      return;
    }
    const captured = await captureOperationResult(
      (work) => perform("import-preview", work, undefined, { backgroundRefresh: true }),
      async () => await relayCommands.remoteAction({ type: "preview_account_batch_import" }, { content }) as ImportSession,
    );
    if (!mounted.current) return;
    if (captured.ok && captured.value) acceptSession(captured.value);
    else if (!captured.ok) setCommandFailed(true);
  };
  const chooseFiles = async (paths?: string[]) => {
    setFileLoading(true);
    let captured: { ok: boolean; value: ImportSession | null | undefined } = { ok: false, value: undefined };
    let createdSessionId: string | null = null;
    try {
      captured = await captureOperationResult(
        (work) => perform("import-files", work, undefined, { backgroundRefresh: true }),
        async () => {
          const session = mode === "local"
            ? await relayCommands.previewImportFiles(paths)
            : await relayCommands.previewRemoteImportFiles(paths);
          if (session) {
            activeSessionId.current = session.sessionId;
            createdSessionId = session.sessionId;
          }
          return session;
        },
      );
      if (!mounted.current) return;
      if (captured.ok && captured.value) acceptSession(captured.value);
      else if (!captured.ok) setCommandFailed(true);
    } finally {
      if (!mounted.current && mode === "local" && createdSessionId) {
        void relayCommands.cancelImport(createdSessionId).catch(() => undefined);
      }
      if (mounted.current) setFileLoading(false);
    }
  };
  const finishConfirmedImport = (result: ConfirmAccountImportResponse | null) => {
    if (!session) return;
    const failures = collectImportFailures(result, session);
    setProgress(null);
    if (failures.length) {
      setSelected(failures.map((failure) => failure.itemId));
      setCompleted(failures);
      return;
    }
    activeSessionId.current = null;
    onImported?.();
    if (mounted.current) onClose();
  };
  const confirm = async (selectedIds = selected) => {
    if (!session || confirmInFlight.current || closing.current) return;
    confirmInFlight.current = true;
    const sessionId = session.sessionId;
    setCommandFailed(false);
    setProgress({ sessionId, completed: 0, total: selectedIds.length, succeeded: 0, failed: 0 });
    try {
      if (mode === "local") {
        let captured: { ok: boolean; value: ConfirmAccountImportResponse | undefined } = { ok: false, value: undefined };
        beginAccountImportConfirmation();
        try {
          captured = await captureOperationResult(
            (work) => perform("import-confirm", work, undefined, { backgroundRefresh: true }),
            () => relayCommands.confirmImport(sessionId, selectedIds, addToPool),
          );
        } finally {
          finishAccountImportConfirmation();
        }
        if (!mounted.current) return;
        if (!captured.ok) {
          setProgress(null);
          setCommandFailed(true);
          return;
        }
        if (assignProxy && captured.value) {
          const accountIds = captured.value.results.flatMap((item) => item.status === "succeeded" && item.account ? [item.account.account.id] : []);
          if (accountIds.length) await perform("import-proxy-assign", () => relayCommands.assignAutomaticProxies(accountIds), undefined, { backgroundRefresh: true });
        }
        if (!mounted.current) return;
        // Clear the cleanup marker before notifying a parent. Onboarding may
        // replace the whole wizard immediately, and its unmount cleanup must
        // not race a confirmation that has already committed.
        finishConfirmedImport(captured.value ?? null);
        return;
      }
      const captured = await captureOperationResult(
        (work) => perform("import-confirm", work, "feedback.accountAdded", { backgroundRefresh: true }),
        async () => await relayCommands.remoteAction(
          { type: "confirm_account_batch_import" },
          { sessionId, selectedItemIds: selectedIds, probeMetadata: true, addToPool },
        ) as ConfirmAccountImportResponse,
      );
      if (!mounted.current) return;
      if (!captured.ok) {
        setProgress(null);
        setCommandFailed(true);
        return;
      }
      finishConfirmedImport(captured.value ?? null);
    } finally {
      confirmInFlight.current = false;
    }
  };
  const retryFailed = () => {
    const failedIds = completed?.map((failure) => failure.itemId) ?? [];
    if (!failedIds.length) return;
    setCompleted(null);
    setSelected(failedIds);
    void confirm(failedIds);
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    if (mode !== "local") return;
    let disposed = false;
    let stop: (() => void) | undefined;
    void relayCommands.onImportProgress((event) => {
      if (event.sessionId === activeSessionId.current) setProgress(event);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, [mode]);
  useEffect(() => {
    if (!initialPaths?.length || initialPreviewStarted.current) return;
    initialPreviewStarted.current = true;
    void chooseFiles(initialPaths);
  }, [initialPaths]);
  useEffect(() => () => {
    if (mode === "local" && activeSessionId.current) {
      void relayCommands.cancelImport(activeSessionId.current).catch(() => undefined);
    }
  }, [mode]);
  const importRows = session?.preview.rows ?? [];
  const toggle = (itemId: string) => setSelected((current) => current.includes(itemId)
    ? current.filter((id) => id !== itemId)
    : [...current, itemId]);
  // Keep invalid rows selectable so the user can explicitly submit them and
  // receive a per-item failure result instead of losing the row silently.
  // Rust still validates the item and never imports unusable credentials.
  const importRowIds = importRows.map((row) => row.itemId);
  const selectedImportRowCount = importRowIds.filter((itemId) => selected.includes(itemId)).length;
  const allImportRowsSelected = importRowIds.length > 0 && selectedImportRowCount === importRowIds.length;
  const someImportRowsSelected = selectedImportRowCount > 0 && !allImportRowsSelected;
  useEffect(() => {
    if (selectAllRef.current) selectAllRef.current.indeterminate = someImportRowsSelected;
  }, [someImportRowsSelected]);
  const toggleAll = (checked: boolean) => setSelected(checked ? importRowIds : []);
  const selectedAccountCount = session?.preview.rows.filter((row) => selected.includes(row.itemId) && row.authMode !== "api_key").length ?? 0;
  const localProxyOptions = mode === "local";
  const footer = completed ? (
    <>
      <Button variant="secondary" disabled={importOperationBusy} onClick={cancel}>{t("common.close")}</Button>
      <Button variant="primary" disabled={importOperationBusy} onClick={retryFailed}>{t("accounts.retryFailed")}</Button>
    </>
  ) : (
    <>
      <Button variant="secondary" disabled={importOperationBusy} onClick={cancel}>{t("common.cancel")}</Button>
      {fileLoading ? null : session ? (
        <Button variant="primary" busy={busy === "import-confirm"} disabled={selected.length === 0 || importOperationBusy} onClick={() => void confirm()}>{t("accounts.confirmImport", { count: selected.length })}</Button>
      ) : (
        <Button variant="primary" busy={busy === "import-preview"} disabled={!content.trim() || importOperationBusy} onClick={preview}>{t("accounts.preview")}</Button>
      )}
    </>
  );
  let body;
  if (busy === "import-confirm" && progress) {
    body = <ImportProgressView mode={mode} progress={progress} />;
  } else if (completed) {
    body = <ImportFailureSummary failures={completed} />;
  } else if (session) {
    body = (
      <ImportPreview
        session={session}
        selected={selected}
        busy={importOperationBusy}
        selectAllRef={selectAllRef}
        allSelected={allImportRowsSelected}
        someSelected={someImportRowsSelected}
        onToggle={toggle}
        onToggleAll={toggleAll}
        canImportToPool={canImportToPool}
        showProxy={localProxyOptions}
        addToPool={addToPool}
        onAddToPool={setAddToPool}
        assignProxy={assignProxy}
        onAssignProxy={setAssignProxy}
        proxyDisabled={!proxyPool || proxyPool.total === 0 || selectedAccountCount === 0}
        proxyDescription={proxyPool ? t(proxyPool.total ? "proxies.importAssignmentHint" : "proxies.noStored", { total: proxyPool.total, selected: selectedAccountCount, count: proxyPool.total }) : t("common.loading")}
      />
    );
  } else if (fileLoading || busy === "import-preview") {
    body = <ImportFileStatus />;
  } else {
    body = <ImportSourceForm mode={mode} choosingFiles={busy === "import-files"} content={content} onContent={setContent} onChooseFiles={() => void chooseFiles()} />;
  }
  return (
    <Dialog className="account-import-dialog" title={t("accounts.import")} onClose={cancel} footer={footer}>
      {commandFailed ? <p role="alert" className="form-note error-text">{t("accounts.importCommandFailed")}</p> : null}
      {body}
    </Dialog>
  );
}

function ImportProgressView({ mode, progress }: { mode: RelayMode; progress: AccountImportProgress }) {
  const { t } = useTranslation();
  return (
    <div className="import-progress" role="status" aria-live="polite">
      <header>
        <span><Loader2 className="spin" aria-hidden /></span>
        <div>
          <strong>{t("accounts.importProgress", { completed: progress.completed, total: progress.total })}</strong>
          <small>{mode === "local" && progress.currentLabel ? t("accounts.importCurrent", { name: progress.currentLabel }) : t("accounts.importProcessing")}</small>
        </div>
        <b>{progress.completed}/{progress.total}</b>
      </header>
      <progress max={Math.max(1, progress.total)} value={mode === "local" ? progress.completed : undefined} />
      {mode === "local" ? <p>{t("accounts.importProgressSummary", { succeeded: progress.succeeded, failed: progress.failed })}</p> : null}
    </div>
  );
}

function ImportFailureSummary({ failures }: { failures: ImportFailure[] }) {
  const { t } = useTranslation();
  return (
    <div role="alert" className="relay-form import-failure-summary">
      <strong>{t("accounts.importIncomplete")}</strong>
      <p>{t("accounts.importIncompleteHint", { count: failures.length })}</p>
      <ul className="import-failure-list">{failures.map((failure) => (
        <li key={failure.itemId}>
          <div>
            <strong>{failure.label || t("accounts.importUnknownAccount")}</strong>
            <code data-relay-tooltip={t("accounts.importTechnicalCode")}>{failure.code}</code>
          </div>
          {failure.identity ? <span>{failure.identity}</span> : null}
          <p>{importFailureReason(failure.code, t)}</p>
        </li>
      ))}</ul>
    </div>
  );
}

function ImportFileStatus() {
  const { t } = useTranslation();
  return (
    <div className="import-file-loading" role="status" aria-live="polite">
      <span><Loader2 className="spin" aria-hidden /></span>
      <div>
        <strong>{t("accounts.readingImportFiles")}</strong>
        <p>{t("accounts.readingImportFilesHint")}</p>
      </div>
    </div>
  );
}

function ImportSourceForm({
  mode,
  choosingFiles,
  content,
  onContent,
  onChooseFiles,
}: {
  mode: RelayMode;
  choosingFiles: boolean;
  content: string;
  onContent: (value: string) => void;
  onChooseFiles: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="relay-form import-start">
      <button type="button" className="import-file-source" disabled={choosingFiles} onClick={onChooseFiles}>
        <span>{choosingFiles ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />}</span>
        <strong>{t("accounts.chooseImportFiles")}</strong>
        <small>{t("accounts.importFileHint")}</small>
      </button>
      <div className="import-source-divider"><span>{t("accounts.orPaste")}</span></div>
      <label className="relay-field">
        <span>{t("accounts.importData")}</span>
        <textarea value={content} onChange={(event) => onContent(event.target.value)} placeholder={mode === "local" ? t("accounts.importPlaceholder") : t("accounts.remoteImportPlaceholder")} spellCheck={false} />
      </label>
      <p className="form-note">{t("accounts.importFormatsHint")}</p>
    </div>
  );
}

type ImportPreviewRowStatus = "ready" | "warning" | "error" | "info";

function importPreviewTone(status: string): ImportPreviewRowStatus {
  if (status === "invalid") return "error";
  if (status === "quota_failed") return "warning";
  if (status === "existing") return "info";
  return "ready";
}

function ImportPreview({
  session,
  selected,
  busy,
  selectAllRef,
  allSelected,
  someSelected,
  onToggle,
  onToggleAll,
  canImportToPool,
  showProxy,
  addToPool,
  onAddToPool,
  assignProxy,
  onAssignProxy,
  proxyDisabled,
  proxyDescription,
}: {
  session: ImportSession;
  selected: string[];
  busy: boolean;
  selectAllRef: RefObject<HTMLInputElement | null>;
  allSelected: boolean;
  someSelected: boolean;
  onToggle: (itemId: string) => void;
  onToggleAll: (checked: boolean) => void;
  canImportToPool: boolean;
  showProxy: boolean;
  addToPool: boolean;
  onAddToPool: (checked: boolean) => void;
  assignProxy: boolean;
  onAssignProxy: (checked: boolean) => void;
  proxyDisabled: boolean;
  proxyDescription: string;
}) {
  const { t } = useTranslation();
  const rows = session.preview.rows;
  const showAfter = canImportToPool || showProxy;
  return <div className="import-preview">
    <div className="import-preview-heading">
      <strong>{t("accounts.importReady")}</strong>
    </div>
    {session.preview.description ? <div className="import-package-description"><span>{t("accounts.importPackageDescription")}</span><MarkdownPreview content={session.preview.description} /></div> : null}
    <ul className="import-account-list">
      <li className="import-account-card import-select-all-row">
        <label>
          <input
            ref={selectAllRef}
            type="checkbox"
            checked={allSelected}
            disabled={!rows.length || busy}
            aria-label={t("accounts.selectAllImport")}
            aria-checked={someSelected ? "mixed" : allSelected ? "true" : "false"}
            onChange={(event) => onToggleAll(event.target.checked)}
          />
          <span className="import-account-copy">
            <strong>{t("accounts.selectAllImport")}</strong>
            <small>{t("accounts.importReadyHint", { selected: selected.length, total: rows.length })}</small>
          </span>
        </label>
      </li>
      {rows.map((row) => {
        const tone = importPreviewTone(row.status);
        const statusLabel = t(`accounts.importStatus.${row.status}`, { defaultValue: row.status });
        const checked = selected.includes(row.itemId);
        return <li className={checked ? "import-account-card selected" : "import-account-card"} key={row.itemId}>
          <label>
            <input type="checkbox" checked={checked} disabled={busy} aria-label={t("accounts.selectImportRow", { name: row.label })} onChange={() => onToggle(row.itemId)} />
            <span className="import-account-copy">
              <span className="import-account-title">
                <strong>{row.label}</strong>
                {tone === "ready" ? null : <span className="import-row-status" data-status={tone}>{statusLabel}</span>}
              </span>
              <span className="import-account-meta">
                <span className="import-account-identity">{row.identity}</span>
                <AccountPlanBadge planType={row.plan ?? null} unknown={t("common.unknown")} />
              </span>
              {row.error ? <small className="import-account-note error-text">{t("accounts.importIssue", { code: row.error.code })}</small> : row.warnings.length ? <small className="import-account-note">{row.warnings.map((warning) => warning.code).join(", ")}</small> : null}
            </span>
          </label>
        </li>;
      })}
    </ul>
    {showAfter ? <section className="import-after settings-group">
      <header><h2>{t("accounts.afterImport")}</h2></header>
      <div className="settings-group-body">
        {canImportToPool ? <SettingToggle className="import-after-toggle" label={t("accounts.addImportedToPool")} description={t("accounts.addToPoolHint")} checked={addToPool} onChange={onAddToPool} /> : null}
        {showProxy ? <SettingToggle className="import-after-toggle" label={t("proxies.assignStoredAfterAdd")} description={proxyDescription} checked={assignProxy} disabled={proxyDisabled} onChange={onAssignProxy} /> : null}
      </div>
    </section> : null}
  </div>;
}

function collectImportFailures(response: ConfirmAccountImportResponse | null, session: ImportSession): ImportFailure[] {
  const rows = new Map(session.preview.rows.map((row) => [row.itemId, row]));
  return (response?.results ?? [])
    .filter((item) => item.status === "failed")
    .map((item) => {
      const row = rows.get(item.itemId);
      return {
        itemId: item.itemId,
        code: item.error?.code ?? "unknown",
        ...(row?.label ? { label: row.label } : {}),
        ...(row?.identity ? { identity: row.identity } : {}),
      };
    });
}

function importFailureReason(code: string, t: TFunction) {
  if (code === "provider_account_id_missing") return t("accounts.importFailureReasons.providerAccountIdMissing");
  if (code === "provider_account_lookup_failed") return t("accounts.importFailureReasons.providerAccountLookupFailed");
  if (code === "access_token_rejected") return t("accounts.importFailureReasons.accessTokenRejected");
  if (code === "account_profile_rate_limited") return t("accounts.importFailureReasons.accountProfileRateLimited");
  if (code === "models_http_status") return t("accounts.importFailureReasons.modelsHttpStatus");
  if (code === "models_forbidden") return t("accounts.importFailureReasons.modelsForbidden");
  return t("accounts.importFailureReasons.unknown");
}
