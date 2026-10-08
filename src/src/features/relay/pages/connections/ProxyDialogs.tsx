import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { TFunction } from "i18next";
import { Check, CircleAlert, CircleCheck, Database, Eye, EyeOff, Globe, Loader2, MapPin, Network, Plus, RefreshCw, Shuffle, Trash2, Upload, UsersRound, WifiOff, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, ProxyAssignmentResult, ProxyPoolEntry, ProxyPoolImportResult, ProxyPoolSummary, StoredProxyAssignmentResult } from "../../api/types";
import { AccountPlanBadge, ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, SecretField, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { captureOperationResult } from "../../state/relayOperationModel";
import { matchesQuery, NoResults } from "./connectionHelpers";
import { useProxyPool } from "./useProxyPool";
import type { ProxyChecks, ProxyCheckState } from "./useProxyChecks";

type AccountProxyChoice = "direct" | "automatic" | "stored" | "custom" | "common";

export function ProxyStorageView({ revision, diagnostics, onImport }: { revision: number; diagnostics: ProxyChecks; onImport: () => void }) {
  const { t, i18n } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const { pool, setPool, failed, load } = useProxyPool(true, revision);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [managedProxyId, setManagedProxyId] = useState<string | null>(null);
  const accountList = runtime?.accounts ?? [];
  const accounts = new Map(accountList.map((account) => [account.id, account]));
  const proxyEntries = (pool?.entries ?? []).filter((proxyEntry) => matchesQuery(
    query,
    proxyEntry.endpoint,
    proxyEntry.countryCode,
    proxyEntry.region,
    proxyEntry.assignedAccountIds.map((accountId) => accounts.get(accountId)?.label ?? t("accounts.importUnknownAccount")),
  ));
  const allSelectableSelected = proxyEntries.length > 0 && proxyEntries.every((proxyEntry) => selected.includes(proxyEntry.id));
  useEffect(() => setSelected((previousSelectedProxyIds) => previousSelectedProxyIds.filter((proxyId) => pool?.entries.some((proxyEntry) => proxyEntry.id === proxyId))), [pool]);
  const remove = async (proxyIds: string[]) => {
    const selectedProxyEntries = (pool?.entries ?? []).filter((proxyEntry) => proxyIds.includes(proxyEntry.id));
    const assignedProxyEntries = selectedProxyEntries.filter((proxyEntry) => proxyEntry.assignedAccountIds.length);
    const assignedAccountIds = new Set(assignedProxyEntries.flatMap((proxyEntry) => proxyEntry.assignedAccountIds));
    const message = assignedProxyEntries.length
      ? t("proxies.deleteAssignedConfirm", { count: proxyIds.length, proxyCount: assignedProxyEntries.length, accountCount: assignedAccountIds.size })
      : t(proxyIds.length === 1 ? "proxies.deleteConfirm" : "proxies.deleteSelectedConfirm", { count: proxyIds.length });
    if (!await confirm(message, { danger: true, ...(assignedProxyEntries.length ? { confirmLabel: t("proxies.detachAndDelete") } : {}) })) return;
    const firstProxyId = proxyIds[0];
    if (!firstProxyId) return;
    const operation = proxyIds.length === 1 ? `proxy-delete-${firstProxyId}` : "proxy-delete-selected";
    const captured = await captureOperationResult(
      (work) => perform(operation, work, "feedback.deleted", { backgroundRefresh: true }),
      async () => {
        for (const proxyEntry of assignedProxyEntries) await relayCommands.setStoredProxyAccounts(proxyEntry.id, []);
        return proxyIds.length === 1
          ? await relayCommands.deleteStoredProxy(firstProxyId)
          : await relayCommands.deleteStoredProxies(proxyIds);
      },
    );
    if (captured.ok && captured.value) {
      setPool(captured.value);
      setSelected([]);
    }
  };
  if (failed) {
    return <EmptyState
      title={t("proxies.storageUnavailable")}
      description={t("proxies.storageUnavailableHint")}
      action={<Button variant="primary" icon={<RefreshCw aria-hidden />} onClick={() => void load()}>{t("common.retry")}</Button>}
    />;
  }
  if (!pool) return <div className="center-loading" role="status"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div>;
  const managedProxy = pool.entries.find((proxyEntry) => proxyEntry.id === managedProxyId) ?? null;
  return <div className="proxy-storage connection-workspace">
    {pool.total ? <div className="table-toolbar proxy-storage-toolbar">
      <div className="proxy-storage-search">
        <label className="proxy-select-all">
          <input
            type="checkbox"
            checked={allSelectableSelected}
            disabled={!proxyEntries.length}
            aria-label={t("proxies.selectAllFree")}
            onChange={(event) => setSelected(event.target.checked ? proxyEntries.map((proxyEntry) => proxyEntry.id) : [])}
          />
        </label>
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("proxies.search")} />
        </label>
      </div>
      {selected.length ? <div className="inline-actions">
        <span className="proxy-selected-count">{t("proxies.selectedCount", { count: selected.length })}</span>
        <Button variant="danger" icon={busy === "proxy-delete-selected" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />} disabled={busy === "proxy-delete-selected"} onClick={() => void remove(selected)}>{t("common.delete")}</Button>
        <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} />
      </div> : <>
        <div className="proxy-storage-counts" aria-label={t("proxies.storageSummary")}>
          <span><small>{t("proxies.total")}</small><strong>{pool.total}</strong></span>
          <span><small>{t("proxies.free")}</small><strong>{pool.free}</strong></span>
          <span><small>{t("proxies.assigned")}</small><strong>{pool.assigned}</strong></span>
        </div>
        <IconButton label={t("common.refresh")} icon={<RefreshCw aria-hidden />} onClick={() => void load()} />
      </>}
    </div> : null}
    {!pool.total ? <EmptyState title={t("proxies.emptyTitle")} description={t("proxies.emptyDescription")} action={<Button variant="primary" icon={<Upload aria-hidden />} onClick={onImport}>{t("proxies.import")}</Button>} />
      : !proxyEntries.length ? <NoResults />
        : <div className="proxy-storage-list" role="list">{proxyEntries.map((proxyEntry) => {
          const assignedNames = proxyEntry.assignedAccountIds.map((accountId) => accounts.get(accountId)?.label ?? t("accounts.importUnknownAccount"));
          return <div className={`proxy-storage-row${selected.includes(proxyEntry.id) ? " selected" : ""}`} role="listitem" key={proxyEntry.id}>
            <label className="proxy-row-select" data-relay-tooltip={t("proxies.selectForDelete")}>
              <input
                type="checkbox"
                checked={selected.includes(proxyEntry.id)}
                aria-label={t("proxies.select", { endpoint: proxyEntry.endpoint })}
                onChange={() => setSelected((previousSelectedProxyIds) => previousSelectedProxyIds.includes(proxyEntry.id) ? previousSelectedProxyIds.filter((selectedProxyId) => selectedProxyId !== proxyEntry.id) : [...previousSelectedProxyIds, proxyEntry.id])}
              />
            </label>
            <div className="proxy-storage-endpoint">
              <div><Network aria-hidden /><strong>{proxyEntry.endpoint}</strong></div>
              {proxyEntry.countryCode || proxyEntry.region ? <small data-relay-tooltip={t("proxies.locationSource")}><MapPin aria-hidden />{t("proxies.declaredLocation", { location: proxyLocationLabel(proxyEntry, i18n.resolvedLanguage ?? i18n.language, t) })}</small> : null}
            </div>
            <ProxyDiagnostic state={diagnostics.checks[proxyEntry.id]} />
            <div className="proxy-storage-account-count" data-relay-tooltip={assignedNames.join(", ")}><span>{assignedNames[0] ?? "-"}</span>{assignedNames.length > 1 ? <small>+{assignedNames.length - 1}</small> : null}</div>
            <div className="row-actions">
              <IconButton label={t("proxies.testConnection")} icon={<Globe aria-hidden />} busy={Boolean(diagnostics.checks[proxyEntry.id]?.pending)} onClick={() => void diagnostics.check(proxyEntry.id)} />
              <IconButton label={t("proxies.manageAccounts")} icon={<UsersRound aria-hidden />} onClick={() => setManagedProxyId(proxyEntry.id)} />
              <ActionMenu>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={busy === `proxy-delete-${proxyEntry.id}` || busy === "proxy-delete-selected"} onClick={() => void remove([proxyEntry.id])}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
            </div>
          </div>;
        })}</div>}
    {managedProxy ? <ProxyAccountsDialog proxyEntry={managedProxy} accounts={accountList} onSaved={setPool} onClose={() => setManagedProxyId(null)} /> : null}
  </div>;
}

function ProxyDiagnostic({ state }: { state: ProxyCheckState | undefined }) {
  const { t, i18n } = useTranslation();
  const proxyCheckResult = state?.result;
  const pending = state?.pending;
  const success = Boolean(proxyCheckResult?.ip && !proxyCheckResult.errorCode);
  const Icon = pending ? Loader2 : success ? CircleCheck : proxyCheckResult ? CircleAlert : Globe;
  const country = proxyCheckResult?.countryCode ? proxyLocationLabel({ countryCode: proxyCheckResult.countryCode, region: null }, i18n.resolvedLanguage ?? i18n.language, t) : null;
  return <div className="proxy-diagnostic" data-state={pending ? "pending" : success ? "success" : proxyCheckResult ? "failed" : "unknown"} role="status">
    <div><Icon className={pending ? "spin" : undefined} aria-hidden /><strong>{t(pending ? "proxies.checking" : success ? "proxies.checkSuccess" : proxyCheckResult ? "proxies.checkFailed" : "proxies.notChecked")}</strong></div>
    {success && proxyCheckResult ? <><code>{proxyCheckResult.ip}</code><small>{[country, t("proxies.latency", { ms: proxyCheckResult.elapsedMs })].filter(Boolean).join(" · ")}</small></> : proxyCheckResult && !pending ? <small>{t(`proxies.checkErrors.${proxyCheckResult.errorCode}`, { defaultValue: t("proxies.checkUnavailable") })}</small> : null}
  </div>;
}

function ProxyAccountsDialog({ proxyEntry, accounts, onSaved, onClose }: { proxyEntry: ProxyPoolEntry; accounts: AccountSummary[]; onSaved: (pool: ProxyPoolSummary) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [selected, setSelected] = useState(proxyEntry.assignedAccountIds);
  const [query, setQuery] = useState("");
  const visible = accounts.filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType));
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.includes(account.id));
  const save = async () => {
    const captured = await captureOperationResult(
      (work) => perform(`proxy-accounts-${proxyEntry.id}`, work, "feedback.saved", { backgroundRefresh: true }),
      () => relayCommands.setStoredProxyAccounts(proxyEntry.id, selected),
    );
    if (captured.ok && captured.value) {
      onSaved(captured.value.pool);
      onClose();
    }
  };
  return <Dialog
    wide
    className="connection-dialog proxy-manager-dialog"
    title={t("proxies.manageAccountsTitle")}
    onClose={onClose}
    footer={<>
      <span className="dialog-selection-count">{t("proxies.assignedCount", { count: selected.length })}</span>
      <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
      <Button variant="primary" busy={busy === `proxy-accounts-${proxyEntry.id}`} onClick={() => void save()}>{t("common.save")}</Button>
    </>}
  >
    <div className="relay-form proxy-account-manager">
      <div className="connection-dialog-context"><Network aria-hidden /><strong>{proxyEntry.endpoint}</strong></div>
      <div className="table-toolbar">
        <label className="toggle-row">
          <input type="checkbox" checked={allSelected} disabled={!accounts.length} onChange={(event) => setSelected(event.target.checked ? accounts.map((account) => account.id) : [])} />
          <span>{t("proxies.selectAll", { count: accounts.length })}</span>
        </label>
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
      </div>
      <div className="scope-grid proxy-account-grid">{visible.map((account) => <label key={account.id}>
        <input type="checkbox" checked={selected.includes(account.id)} onChange={() => setSelected((previousSelectedAccountIds) => previousSelectedAccountIds.includes(account.id) ? previousSelectedAccountIds.filter((selectedAccountId) => selectedAccountId !== account.id) : [...previousSelectedAccountIds, account.id])} />
        <span className="proxy-account-identity" data-relay-tooltip={account.label}><strong>{account.label}</strong></span>
        <AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} />
      </label>)}</div>
      {!visible.length ? <NoResults /> : null}
      <p className="form-note">{t("proxies.sharedProxyHint")}</p>
    </div>
  </Dialog>;
}

function proxyLocationLabel(proxyEntry: Pick<ProxyPoolEntry, "countryCode" | "region">, language: string, t: TFunction) {
  let country = proxyEntry.countryCode;
  if (country) {
    try {
      country = new Intl.DisplayNames([language], { type: "region" }).of(country) ?? country;
    } catch {
      // Keep the declared country code when the runtime cannot localize it.
    }
  }
  return [country, proxyEntry.region ? t("proxies.regionValue", { region: proxyEntry.region }) : null].filter(Boolean).join(" · ") || t("proxies.locationUnknown");
}

export function ProxyImportDialog({ diagnostics, onImported, onClose }: { diagnostics: ProxyChecks; onImported: () => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [importResult, setImportResult] = useState<ProxyPoolImportResult | null>(null);
  const [checkAfterImport, setCheckAfterImport] = useState(true);
  const proxyUrls = content.split(/\r?\n/).map((proxyLine) => proxyLine.trim()).filter(Boolean);
  const importProxies = async () => {
    const captured = await captureOperationResult(
      (work) => perform("proxy-import", work, "feedback.saved", { backgroundRefresh: true }),
      () => relayCommands.importProxyPool(proxyUrls),
    );
    if (!captured.ok || !captured.value) return;
    const importResult = captured.value;
    setImportResult(importResult);
    setContent("");
    onImported();
    if (checkAfterImport) void diagnostics.checkMany(importResult.addedProxyIds);
  };
  const importedProxyEntries = importResult?.pool.entries.filter((proxyEntry) => importResult.addedProxyIds.includes(proxyEntry.id)) ?? [];
  return <Dialog
    className="connection-dialog proxy-import-dialog"
    title={t("proxies.importTitle")}
    onClose={onClose}
    footer={importResult ? <>
      <Button variant="secondary" onClick={() => setImportResult(null)}>{t("proxies.addMore")}</Button>
      <Button variant="primary" onClick={onClose}>{t("common.done")}</Button>
    </> : <>
      <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
      <Button variant="primary" icon={<Upload aria-hidden />} busy={busy === "proxy-import"} disabled={!proxyUrls.length} onClick={() => void importProxies()}>{t("proxies.importCount", { count: proxyUrls.length })}</Button>
    </>}
  >
    <div className="relay-form proxy-import-form">
      {importResult ? <>
        <p className="connection-success" role="status"><CircleCheck aria-hidden />{t("proxies.importResult", importResult)}</p>
        <div className="proxy-import-results">{importedProxyEntries.map((proxyEntry) => <div key={proxyEntry.id}>
          <strong>{proxyEntry.endpoint}</strong>
          <ProxyDiagnostic state={diagnostics.checks[proxyEntry.id]} />
          <IconButton label={t("proxies.testConnection")} icon={<Globe aria-hidden />} busy={Boolean(diagnostics.checks[proxyEntry.id]?.pending)} onClick={() => void diagnostics.check(proxyEntry.id)} />
        </div>)}</div>
      </> : <>
        <p className="connection-dialog-description">{t("proxies.importHint")}</p>
        <label className="relay-field">
          <span>{t("proxies.proxyList")}</span>
          <div className="proxy-list-field">
            <textarea className={revealed ? "" : "secret-textarea"} value={content} onChange={(event) => setContent(event.target.value)} placeholder={t("proxies.proxyListPlaceholder")} autoComplete="off" spellCheck={false} />
            <IconButton type="button" label={revealed ? t("common.hide") : t("common.reveal")} icon={revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setRevealed((isRevealed) => !isRevealed)} />
          </div>
        </label>
        <div className="proxy-format-line"><code>host:port:user:pass</code><code>user:pass@host:port</code><code>http(s)://...</code></div>
        <label className="proxy-check-option">
          <input type="checkbox" checked={checkAfterImport} onChange={(event) => setCheckAfterImport(event.target.checked)} />
          <span><strong>{t("proxies.checkAfterImport")}</strong><small>{t("proxies.checkHint")}</small></span>
        </label>
      </>}
    </div>
  </Dialog>;
}

export function AccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { mode } = useRelayState();
  return mode === "local" ? <LocalAccountProxyDialog account={account} onClose={onClose} /> : <RemoteAccountProxyDialog account={account} onClose={onClose} />;
}

function LocalAccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform, runtime } = useRelayState();
  const { pool } = useProxyPool();
  const [choice, setChoice] = useState<AccountProxyChoice>(() => account.proxyMode === "common" ? "common" : account.proxyMode === "account" ? "stored" : "direct");
  const [proxyId, setProxyId] = useState("");
  const [proxyUrl, setProxyUrl] = useState("");
  const [unavailable, setUnavailable] = useState(false);
  const initialized = useRef(false);
  const assignedProxy = pool?.entries.find((proxyEntry) => proxyEntry.assignedAccountIds.includes(account.id));
  const availableProxies = pool?.entries ?? [];
  useEffect(() => {
    if (!pool || initialized.current) return;
    initialized.current = true;
    if (assignedProxy) {
      setChoice("stored");
      setProxyId(assignedProxy.id);
    } else if (account.proxyMode === "account") {
      setChoice("custom");
    }
  }, [account.proxyMode, assignedProxy, pool]);
  const apply = async () => {
    const captured = await captureOperationResult(
      (work) => perform(`proxy-${account.id}`, work, "feedback.saved", { backgroundRefresh: true }),
      async () => {
        if (choice === "direct") {
          await relayCommands.setAccountProxy(account.id, null, true);
          return null;
        }
        if (choice === "common") {
          await relayCommands.setAccountProxy(account.id, null);
          return null;
        }
        if (choice === "automatic") return relayCommands.assignAutomaticProxies([account.id]);
        if (choice === "stored") return relayCommands.assignStoredProxy(account.id, proxyId);
        await relayCommands.setAccountProxy(account.id, proxyUrl.trim());
        return null;
      },
    );
    if (!captured.ok) return;
    if (captured.value?.unavailable) {
      setUnavailable(true);
      return;
    }
    onClose();
  };
  const directBlocked = Boolean(runtime?.gateway.accountProxyRequired);
  const commonConfigured = Boolean(runtime?.gateway.commonProxyConfigured);
  const valid = Boolean(pool)
    && (choice !== "direct" || !directBlocked)
    && (choice !== "common" || commonConfigured)
    && (choice !== "stored" || proxyId)
    && (choice !== "custom" || proxyUrl.trim())
    && (choice !== "automatic" || pool!.total > 0 || Boolean(assignedProxy));
  const selectChoice = (selectedChoice: AccountProxyChoice) => { setChoice(selectedChoice); setUnavailable(false); };
  return (
    <Dialog
      title={t("proxies.accountTitle")}
      onClose={onClose}
      footer={<>
        <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
        <Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!valid} onClick={() => void apply()}>{t("common.save")}</Button>
      </>}
    >
      <div className="relay-form proxy-route-form">
        {!pool ? <div className="center-loading"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div> : <>
          <p className="proxy-account-context">{account.label}</p>
          <div className="proxy-route-options" role="radiogroup" aria-label={t("proxies.accountRoute")}>
            <ProxyRouteOption
              choice="direct"
              selected={choice === "direct"}
              disabled={directBlocked}
              icon={<WifiOff aria-hidden />}
              label={t("proxies.direct")}
              hint={t(directBlocked ? "proxies.directBlockedHint" : "proxies.directHint")}
              onSelect={selectChoice}
            />
            <ProxyRouteOption
              choice="automatic"
              selected={choice === "automatic"}
              disabled={!pool.total && !assignedProxy}
              icon={<Shuffle aria-hidden />}
              label={t("proxies.assignAutomatically")}
              hint={t("proxies.storedAvailable", { count: pool.total })}
              onSelect={selectChoice}
            />
            <ProxyRouteOption
              choice="stored"
              selected={choice === "stored"}
              disabled={!availableProxies.length}
              icon={<Database aria-hidden />}
              label={t("proxies.chooseStored")}
              hint={t("proxies.chooseStoredShortHint")}
              onSelect={(selectedChoice) => {
                selectChoice(selectedChoice);
                setProxyId((currentId) => currentId || availableProxies[0]?.id || "");
              }}
            />
            <ProxyRouteOption
              choice="custom"
              selected={choice === "custom"}
              icon={<Plus aria-hidden />}
              label={t("proxies.addCustom")}
              hint={t("proxies.addCustomShortHint")}
              onSelect={selectChoice}
            />
            {commonConfigured ? <ProxyRouteOption choice="common" selected={choice === "common"} icon={<Network aria-hidden />} label={t("proxies.useCommon")} hint={t("proxies.useCommonHint")} onSelect={selectChoice} /> : null}
          </div>
          {choice === "stored" && availableProxies.length ? <div className="proxy-route-control">
            <OptionMenu className="field-option-menu" label={t("proxies.chooseStored")} value={proxyId || availableProxies[0]?.id || ""} onChange={setProxyId} options={availableProxies.map((proxyEntry) => ({ value: proxyEntry.id, label: proxyEntry.endpoint }))} />
          </div> : null}
          {choice === "custom" ? <div className="proxy-route-control">
            <SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} />
          </div> : null}
        </>}
        {unavailable ? <p role="alert" className="form-note error-text">{t("proxies.noStoredProxy")}</p> : null}
      </div>
    </Dialog>
  );
}

function RemoteAccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform, runtime } = useRelayState();
  const [choice, setChoice] = useState<AccountProxyChoice>(() => account.proxyMode === "common" ? "common" : account.proxyMode === "account" ? "custom" : "direct");
  const [proxyUrl, setProxyUrl] = useState("");
  const commonConfigured = Boolean(runtime?.gateway.commonProxyConfigured);
  const directBlocked = Boolean(runtime?.gateway.accountProxyRequired);
  const apply = async () => {
    const ok = await perform(
      `proxy-${account.id}`,
      () => relayCommands.remoteAction({ type: "set_account_proxy", id: account.id }, { proxyUrl: choice === "custom" ? proxyUrl.trim() : null, bypassCommonProxy: choice === "direct" }),
      "feedback.saved",
      { backgroundRefresh: true },
    );
    if (ok) onClose();
  };
  const valid = (choice !== "direct" || !directBlocked) && (choice !== "common" || commonConfigured) && (choice !== "custom" || Boolean(proxyUrl.trim()));
  return <Dialog
    title={t("proxies.accountTitle")}
    onClose={onClose}
    footer={<>
      <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
      <Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!valid} onClick={() => void apply()}>{t("common.save")}</Button>
    </>}
  ><div className="relay-form proxy-route-form">
    <p className="proxy-account-context">{account.label}</p>
    <div className="proxy-route-options" role="radiogroup" aria-label={t("proxies.accountRoute")}>
      <ProxyRouteOption
        choice="direct"
        selected={choice === "direct"}
        disabled={directBlocked}
        icon={<WifiOff aria-hidden />}
        label={t("proxies.direct")}
        hint={t(directBlocked ? "proxies.directBlockedHint" : "proxies.directHint")}
        onSelect={setChoice}
      />
      <ProxyRouteOption
        choice="custom"
        selected={choice === "custom"}
        icon={<Plus aria-hidden />}
        label={t("proxies.addCustom")}
        hint={t("proxies.addCustomShortHint")}
        onSelect={setChoice}
      />
      {commonConfigured ? <ProxyRouteOption choice="common" selected={choice === "common"} icon={<Network aria-hidden />} label={t("proxies.useCommon")} hint={t("proxies.useCommonHint")} onSelect={setChoice} /> : null}
    </div>
    {choice === "custom" ? <div className="proxy-route-control">
      <SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} />
      <p className="form-note">{t("proxies.savedHidden")}</p>
    </div> : null}
  </div></Dialog>;
}

function ProxyRouteOption({
  choice,
  selected,
  disabled = false,
  icon,
  label,
  hint,
  onSelect,
}: {
  choice: AccountProxyChoice;
  selected: boolean;
  disabled?: boolean;
  icon: ReactNode;
  label: string;
  hint: string;
  onSelect: (selectedChoice: AccountProxyChoice) => void;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      className={selected ? "selected" : ""}
      onClick={() => onSelect(choice)}
    >
      {icon}
      <span><strong>{label}</strong><small>{hint}</small></span>
      {selected ? <Check className="proxy-route-check" aria-hidden /> : null}
    </button>
  );
}

export function BulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { mode } = useRelayState();
  return mode === "local" ? <LocalBulkProxyDialog accountIds={accountIds} onClose={onClose} /> : <RemoteBulkProxyDialog accountIds={accountIds} onClose={onClose} />;
}

function LocalBulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool, setPool } = useProxyPool();
  const [assignmentResult, setAssignmentResult] = useState<StoredProxyAssignmentResult | null>(null);
  const accounts = (runtime?.accounts ?? []).filter((account) => accountIds.includes(account.id));
  const needProxy = accounts.filter((account) => account.proxyMode !== "account").length;
  const assign = async () => {
    const captured = await captureOperationResult(
      (work) => perform("proxy-bulk", work, "feedback.saved", { backgroundRefresh: true }),
      () => relayCommands.assignAutomaticProxies(accounts.map((account) => account.id)),
    );
    if (captured.ok && captured.value) {
      setAssignmentResult(captured.value);
      setPool(captured.value.pool);
    }
  };
  return <Dialog
    className="connection-dialog proxy-bulk-dialog"
    title={t("proxies.bulkTitle")}
    onClose={onClose}
    footer={<>
      <Button variant="secondary" onClick={onClose}>{assignmentResult ? t("common.done") : t("common.cancel")}</Button>
      <Button variant="primary" busy={busy === "proxy-bulk"} disabled={!pool || !accounts.length || (needProxy > 0 && pool.total === 0)} onClick={() => void assign()}>{t("proxies.assignAutomatically")}</Button>
    </>}
  >
    <div className="relay-form">
      <div className="proxy-assignment-summary">
        <div><span>{t("connections.accounts")}</span><strong>{accounts.length}</strong></div>
        <div><span>{t("proxies.needProxy")}</span><strong>{needProxy}</strong></div>
        <div><span>{t("proxies.total")}</span><strong>{pool?.total ?? "-"}</strong></div>
      </div>
      <p className="form-note">{t("proxies.bulkStoredHint")}</p>
      {pool && needProxy > 0 && pool.total === 0 ? <p className="form-note warning-text">{t("proxies.noStored")}</p> : null}
      {assignmentResult ? <p role="status" className="form-note success-text">{t("proxies.bulkStoredResult", assignmentResult)}</p> : null}
    </div>
  </Dialog>;
}

function RemoteBulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const accountById = new Map((runtime?.accounts ?? []).map((account) => [account.id, account]));
  const accounts = accountIds.map((accountId) => accountById.get(accountId)).filter((account): account is AccountSummary => Boolean(account));
  const [selected, setSelected] = useState(() => accounts.map((account) => account.id));
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [assignmentResult, setAssignmentResult] = useState<ProxyAssignmentResult | null>(null);
  const proxyUrls = content.split(/\r?\n/).map((proxyLine) => proxyLine.trim()).filter(Boolean);
  const selectedAccountIds = accounts.filter((account) => selected.includes(account.id)).map((account) => account.id);
  const valid = selectedAccountIds.length > 0 && proxyUrls.length >= selectedAccountIds.length;
  const toggle = (accountId: string) => setSelected((previousSelectedAccountIds) => previousSelectedAccountIds.includes(accountId) ? previousSelectedAccountIds.filter((selectedAccountId) => selectedAccountId !== accountId) : [...previousSelectedAccountIds, accountId]);
  const assign = async () => {
    const captured = await captureOperationResult(
      (work) => perform("proxy-bulk", work, "feedback.saved", { backgroundRefresh: true }),
      async () => await relayCommands.remoteAction({ type: "assign_account_proxies" }, { accountIds: selectedAccountIds, proxyUrls }) as ProxyAssignmentResult,
    );
    if (captured.ok) {
      setAssignmentResult(captured.value ?? null);
      setContent("");
    }
  };
  return <Dialog
    wide
    className="connection-dialog proxy-bulk-dialog"
    title={t("proxies.bulkTitle")}
    onClose={onClose}
    footer={<>
      <Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>
      <Button variant="primary" busy={busy === "proxy-bulk"} disabled={!valid} onClick={assign}>{t("proxies.assign")}</Button>
    </>}
  >
    <div className="relay-form">
      <label className="toggle-row">
        <input type="checkbox" checked={selectedAccountIds.length === accounts.length && accounts.length > 0} onChange={(event) => setSelected(event.target.checked ? accounts.map((account) => account.id) : [])} />
        <span>{t("proxies.selectAll", { count: accounts.length })}</span>
      </label>
      <fieldset>
        <legend>{t("connections.accounts")}</legend>
        <div className="scope-grid proxy-account-grid">{accounts.map((account) => <label key={account.id}>
          <input type="checkbox" checked={selected.includes(account.id)} onChange={() => toggle(account.id)} />
          {account.label}
        </label>)}</div>
      </fieldset>
      <label className="relay-field">
        <span>{t("proxies.proxyList")}</span>
        <div className="proxy-list-field">
          <textarea className={revealed ? "" : "secret-textarea"} value={content} onChange={(event) => { setContent(event.target.value); setAssignmentResult(null); }} placeholder={t("proxies.proxyListPlaceholder")} autoComplete="off" spellCheck={false} />
          <IconButton type="button" label={revealed ? t("common.hide") : t("common.reveal")} icon={revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setRevealed((isRevealed) => !isRevealed)} />
        </div>
      </label>
      <p className="form-note">{t("proxies.bulkHint", { selected: selectedAccountIds.length, provided: proxyUrls.length })}</p>
      {assignmentResult ? <p role="status" className="form-note success-text">{t("proxies.bulkResult", assignmentResult)}</p> : null}
    </div>
  </Dialog>;
}
