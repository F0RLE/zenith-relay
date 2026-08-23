import { useEffect, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { Loader2, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountImportProgress, ConfirmAccountImportResponse, ImportSession, RelayMode } from "../../api/types";
import { AccountPlanBadge, Button, Dialog, StatusBadge, StatusIcon } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { MarkdownPreview } from "./MarkdownPreview";
import { useProxyPool } from "./ProxyDialogs";

type ImportFailure = { itemId: string; code: string; label?: string; identity?: string };

function selectedImportItemIds(session?: ImportSession) {
  return session?.preview.rows
    .filter((row) => row.selectable && row.defaultSelected)
    .map((row) => row.itemId) ?? [];
}

export function ImportDialog({ initialPaths, initialSession, modeOverride, defaultAddToPool = false, onImported, onClose }: { initialPaths?: string[]; initialSession?: ImportSession; modeOverride?: RelayMode; defaultAddToPool?: boolean; onImported?: () => void; onClose: () => void }) {
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
  const initialPreviewStarted = useRef(false);
  const canImportToPool = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("account_import_to_pool"));
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
    const sessionId = session?.sessionId ?? ownedSessionId;
    if (mode === "local" && sessionId) await perform("import-cancel", () => relayCommands.cancelImport(sessionId));
    activeSessionId.current = null;
    onClose();
  };
  const preview = async () => {
    if (mode === "local") {
      const result: { current: ImportSession | null } = { current: null };
      const ok = await perform("import-preview", async () => {
        const started = await relayCommands.startImport(content);
        setOwnedSessionId(started.sessionId);
        result.current = await relayCommands.prepareImport(started.sessionId, false);
      });
      if (ok && result.current) acceptSession(result.current);
      else if (!ok) setCommandFailed(true);
      return;
    }
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("import-preview", async () => {
      result.current = await relayCommands.remoteAction({ type: "preview_account_batch_import" }, { content }) as ImportSession;
    });
    if (ok && result.current) acceptSession(result.current);
    else if (!ok) setCommandFailed(true);
  };
  const chooseFiles = async (paths?: string[]) => {
    setFileLoading(true);
    const result: { current: ImportSession | null } = { current: null };
    try {
      const ok = await perform("import-files", async () => {
        result.current = mode === "local"
          ? await relayCommands.previewImportFiles(paths)
          : await relayCommands.previewRemoteImportFiles(paths);
      });
      if (ok && result.current) acceptSession(result.current);
      else if (!ok) setCommandFailed(true);
    } finally {
      setFileLoading(false);
    }
  };
  const confirm = async (selectedIds = selected) => {
    if (!session) return;
    setCommandFailed(false);
    setProgress({ sessionId: session.sessionId, completed: 0, total: selectedIds.length, succeeded: 0, failed: 0 });
    if (mode === "local") {
      const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
      const ok = await perform("import-confirm", async () => {
        result.current = await relayCommands.confirmImport(session.sessionId, selectedIds, addToPool);
      });
      if (!ok) {
        setProgress(null);
        setCommandFailed(true);
        return;
      }
      if (assignProxy && result.current) {
        const accountIds = result.current.results.flatMap((item) => item.status === "succeeded" && item.account ? [item.account.account.id] : []);
        if (accountIds.length) await perform("import-proxy-assign", () => relayCommands.assignAutomaticProxies(accountIds));
      }
      const failures = collectImportFailures(result.current, session);
      if (result.current?.results.some((item) => item.status === "succeeded")) onImported?.();
      setProgress(null);
      if (failures.length) {
        setSelected(failures.map((failure) => failure.itemId));
        setCompleted(failures);
        return;
      }
      activeSessionId.current = null;
      onClose();
      return;
    }
    const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
    const ok = await perform("import-confirm", async () => {
      result.current = await relayCommands.remoteAction(
        { type: "confirm_account_batch_import" },
        { sessionId: session.sessionId, selectedItemIds: selectedIds, probeMetadata: true, addToPool },
      ) as Awaited<ReturnType<typeof relayCommands.confirmImport>>;
    }, "feedback.accountAdded");
    if (!ok) {
      setProgress(null);
      setCommandFailed(true);
      return;
    }
    const failures = collectImportFailures(result.current, session);
    if (result.current?.results.some((item) => item.status === "succeeded")) onImported?.();
    setProgress(null);
    if (failures.length) {
      setSelected(failures.map((failure) => failure.itemId));
      setCompleted(failures);
    } else {
      activeSessionId.current = null;
      onClose();
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
  const toggle = (itemId: string) => setSelected((current) => current.includes(itemId)
    ? current.filter((id) => id !== itemId)
    : [...current, itemId]);
  const selectedAccountCount = session?.preview.rows.filter((row) => selected.includes(row.itemId) && row.authMode !== "api_key").length ?? 0;
  const localProxyOptions = mode === "local";
  const footer = completed
    ? <><Button variant="secondary" onClick={cancel}>{t("common.close")}</Button><Button variant="primary" onClick={retryFailed}>{t("accounts.retryFailed")}</Button></>
    : <><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{fileLoading ? null : session ? <Button variant="primary" busy={busy === "import-confirm"} disabled={selected.length === 0} onClick={() => void confirm()}>{t("accounts.confirmImport", { count: selected.length })}</Button> : <Button variant="primary" busy={busy === "import-preview"} disabled={!content.trim()} onClick={preview}>{t("accounts.preview")}</Button>}</>;
  const body = busy === "import-confirm" && progress ? <div className="import-progress" role="status" aria-live="polite"><header><span><Loader2 className="spin" aria-hidden /></span><div><strong>{t("accounts.importProgress", { completed: progress.completed, total: progress.total })}</strong><small>{mode === "local" && progress.currentLabel ? t("accounts.importCurrent", { name: progress.currentLabel }) : t("accounts.importProcessing")}</small></div><b>{progress.completed}/{progress.total}</b></header><progress max={Math.max(1, progress.total)} value={mode === "local" ? progress.completed : undefined} />{mode === "local" ? <p>{t("accounts.importProgressSummary", { succeeded: progress.succeeded, failed: progress.failed })}</p> : null}</div> : completed ? <div role="alert" className="relay-form import-failure-summary"><strong>{t("accounts.importIncomplete")}</strong><p>{t("accounts.importIncompleteHint", { count: completed.length })}</p><ul className="import-failure-list">{completed.map((failure) => <li key={failure.itemId}><div><strong>{failure.label || t("accounts.importUnknownAccount")}</strong><code title={t("accounts.importTechnicalCode")}>{failure.code}</code></div>{failure.identity ? <span>{failure.identity}</span> : null}<p>{importFailureReason(failure.code, t)}</p></li>)}</ul></div> : session ? <div className="import-preview"><div className="import-preview-heading"><div><strong>{t("accounts.importReady")}</strong><span>{t("accounts.importReadyHint", { selected: selected.length, total: session.preview.rows.length })}</span></div><StatusBadge status={selected.length ? "ready" : "warning"} label={t("accounts.selectedCount", { count: selected.length })} /></div>{session.preview.description ? <div className="import-package-description"><span>{t("accounts.importPackageDescription")}</span><MarkdownPreview content={session.preview.description} /></div> : null}<div className="relay-table-wrap"><table className="relay-table"><thead><tr><th><span className="sr-only">{t("accounts.selectImport")}</span></th><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("accounts.identity")}</th><th>{t("accounts.plan")}</th></tr></thead><tbody>{session.preview.rows.map((row) => {
    const badge = row.status === "invalid" ? "error" : row.status === "quota_failed" ? "warning" : row.status === "existing" ? "info" : "ready";
    return <tr key={row.itemId}><td><input type="checkbox" checked={selected.includes(row.itemId)} disabled={!row.selectable} aria-label={t("accounts.selectImportRow", { name: row.label })} onChange={() => toggle(row.itemId)} /></td><td><StatusIcon status={badge} label={t(`accounts.importStatus.${row.status}`, { defaultValue: row.status })} /></td><td>{row.label}{row.error ? <small className="error-text">{t("accounts.importIssue", { code: row.error.code })}</small> : row.warnings.length ? <small>{row.warnings.map((warning) => warning.code).join(", ")}</small> : null}</td><td><code>{row.identity}</code></td><td><AccountPlanBadge planType={row.plan ?? null} unknown="-" /></td></tr>;
  })}</tbody></table></div>{canImportToPool || localProxyOptions ? <div className="post-import-options"><span>{t("accounts.afterImport")}</span>{canImportToPool ? <label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addImportedToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label> : null}{localProxyOptions ? <label><input type="checkbox" checked={assignProxy} disabled={!proxyPool || proxyPool.total === 0 || selectedAccountCount === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignStoredAfterAdd")}</strong><small>{proxyPool ? t(proxyPool.total ? "proxies.importAssignmentHint" : "proxies.noStored", { total: proxyPool.total, selected: selectedAccountCount, count: proxyPool.total }) : t("common.loading")}</small></span></label> : null}</div> : null}</div> : fileLoading || busy === "import-preview" ? <div className="import-file-loading" role="status" aria-live="polite"><span><Loader2 className="spin" aria-hidden /></span><div><strong>{t("accounts.readingImportFiles")}</strong><p>{t("accounts.readingImportFilesHint")}</p></div></div> : <div className="relay-form import-start"><button type="button" className="import-file-source" disabled={busy === "import-files"} onClick={() => void chooseFiles()}><span>{busy === "import-files" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />}</span><strong>{t("accounts.chooseImportFiles")}</strong><small>{t("accounts.importFileHint")}</small></button><div className="import-source-divider"><span>{t("accounts.orPaste")}</span></div><label className="relay-field"><span>{t("accounts.importData")}</span><textarea value={content} onChange={(event) => setContent(event.target.value)} placeholder={mode === "local" ? t("accounts.importPlaceholder") : t("accounts.remoteImportPlaceholder")} spellCheck={false} /></label><p className="form-note">{t("accounts.importFormatsHint")}</p></div>;
  return <Dialog wide title={t("accounts.import")} onClose={cancel} footer={footer}>{commandFailed ? <p role="alert" className="form-note error-text">{t("accounts.importCommandFailed")}</p> : null}{body}</Dialog>;
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
        label: row?.label,
        identity: row?.identity,
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
