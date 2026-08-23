import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { TFunction } from "i18next";
import { Database, Eye, EyeOff, Loader2, MapPin, Network, Plus, RefreshCw, Shuffle, Trash2, Upload, UsersRound, WifiOff, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, ProxyAssignmentResult, ProxyPoolEntry, ProxyPoolSummary, StoredProxyAssignmentResult } from "../../api/types";
import { AccountPlanBadge, Button, Dialog, EmptyState, IconButton, OptionMenu, SecretField, StatusBadge, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { matchesQuery, NoResults } from "./connectionHelpers";

type AccountProxyChoice = "direct" | "automatic" | "stored" | "custom" | "common";

export function useProxyPool(enabled = true, revision = 0) {
  const [pool, setPool] = useState<ProxyPoolSummary | null>(null);
  const [failed, setFailed] = useState(false);
  const load = useCallback(async () => {
    if (!enabled) return;
    try {
      setPool(await relayCommands.getProxyPool());
      setFailed(false);
    } catch {
      setFailed(true);
    }
  }, [enabled]);
  useEffect(() => { void load(); }, [load, revision]);
  return { pool, setPool, failed, load };
}

export function ProxyStorageView({ revision, onImport }: { revision: number; onImport: () => void }) {
  const { t, i18n } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const { pool, setPool, failed, load } = useProxyPool(true, revision);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [managedProxyId, setManagedProxyId] = useState<string | null>(null);
  const accountList = runtime?.accounts ?? [];
  const accounts = new Map(accountList.map((account) => [account.id, account]));
  const entries = (pool?.entries ?? []).filter((entry) => matchesQuery(
    query,
    entry.endpoint,
    entry.countryCode,
    entry.region,
    entry.assignedAccountIds.map((accountId) => accounts.get(accountId)?.label ?? t("accounts.importUnknownAccount")),
  ));
  const selectable = entries;
  const allSelectableSelected = selectable.length > 0 && selectable.every((entry) => selected.includes(entry.id));
  useEffect(() => setSelected((current) => current.filter((proxyId) => pool?.entries.some((entry) => entry.id === proxyId))), [pool]);
  const remove = async (proxyIds: string[]) => {
    const selectedEntries = (pool?.entries ?? []).filter((entry) => proxyIds.includes(entry.id));
    const assignedEntries = selectedEntries.filter((entry) => entry.assignedAccountIds.length);
    const assignedAccounts = new Set(assignedEntries.flatMap((entry) => entry.assignedAccountIds));
    const message = assignedEntries.length
      ? t("proxies.deleteAssignedConfirm", { count: proxyIds.length, proxyCount: assignedEntries.length, accountCount: assignedAccounts.size })
      : t(proxyIds.length === 1 ? "proxies.deleteConfirm" : "proxies.deleteSelectedConfirm", { count: proxyIds.length });
    if (!await confirm(message, { danger: true, confirmLabel: assignedEntries.length ? t("proxies.detachAndDelete") : undefined })) return;
    let next: ProxyPoolSummary | null = null;
    const operation = proxyIds.length === 1 ? `proxy-delete-${proxyIds[0]}` : "proxy-delete-selected";
    const ok = await perform(operation, async () => {
      for (const entry of assignedEntries) await relayCommands.setStoredProxyAccounts(entry.id, []);
      next = proxyIds.length === 1
        ? await relayCommands.deleteStoredProxy(proxyIds[0])
        : await relayCommands.deleteStoredProxies(proxyIds);
    }, "feedback.deleted");
    if (ok && next) {
      setPool(next);
      setSelected([]);
    }
  };
  if (failed) return <EmptyState title={t("proxies.storageUnavailable")} description={t("proxies.storageUnavailableHint")} action={<Button variant="primary" icon={<RefreshCw aria-hidden />} onClick={() => void load()}>{t("common.retry")}</Button>} />;
  if (!pool) return <div className="center-loading" role="status"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div>;
  const managedProxy = pool.entries.find((entry) => entry.id === managedProxyId) ?? null;
  return <div className="proxy-storage">
    {pool.total ? <div className="table-toolbar proxy-storage-toolbar"><div className="proxy-storage-search"><label className="proxy-select-all"><input type="checkbox" checked={allSelectableSelected} disabled={!selectable.length} aria-label={t("proxies.selectAllFree")} onChange={(event) => setSelected(event.target.checked ? selectable.map((entry) => entry.id) : [])} /></label><label className="search-field"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("proxies.search")} /></label></div>{selected.length ? <div className="inline-actions"><span className="proxy-selected-count">{t("proxies.selectedCount", { count: selected.length })}</span><Button variant="danger" icon={busy === "proxy-delete-selected" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={() => void remove(selected)}>{t("common.delete")}</Button><IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} /></div> : <><div className="proxy-storage-counts" aria-label={t("proxies.storageSummary")}><span><small>{t("proxies.total")}</small><strong>{pool.total}</strong></span><span><small>{t("proxies.free")}</small><strong>{pool.free}</strong></span><span><small>{t("proxies.assigned")}</small><strong>{pool.assigned}</strong></span></div><IconButton label={t("common.refresh")} icon={<RefreshCw aria-hidden />} onClick={() => void load()} /></>}</div> : null}
    {!pool.total ? <EmptyState title={t("proxies.emptyTitle")} description={t("proxies.emptyDescription")} action={<Button variant="primary" icon={<Upload aria-hidden />} onClick={onImport}>{t("proxies.import")}</Button>} />
      : !entries.length ? <NoResults />
        : <div className="proxy-storage-list" role="list"><div className="proxy-storage-head" aria-hidden><span /><span>{t("proxies.endpoint")}</span><span>{t("proxies.assignedAccounts")}</span><span>{t("common.status")}</span><span /></div>{entries.map((entry) => {
          const assignedNames = entry.assignedAccountIds.map((accountId) => accounts.get(accountId)?.label ?? t("accounts.importUnknownAccount"));
          const assigned = assignedNames.length > 0;
          return <div className={`proxy-storage-row${selected.includes(entry.id) ? " selected" : ""}`} role="listitem" key={entry.id}>
            <label className="proxy-row-select" title={t("proxies.selectForDelete")}><input type="checkbox" checked={selected.includes(entry.id)} aria-label={t("proxies.select", { endpoint: entry.endpoint })} onChange={() => setSelected((current) => current.includes(entry.id) ? current.filter((id) => id !== entry.id) : [...current, entry.id])} /></label>
            <div className="proxy-storage-endpoint"><div><Network aria-hidden /><strong>{entry.endpoint}</strong></div><small title={t("proxies.locationSource")}><MapPin aria-hidden />{proxyLocationLabel(entry, i18n.resolvedLanguage ?? i18n.language, t)}</small></div>
            <div className="proxy-storage-account-count" title={assignedNames.join(", ")}><span>{assignedNames[0] ?? "-"}</span>{assignedNames.length > 1 ? <small>+{assignedNames.length - 1}</small> : null}</div>
            <StatusBadge status={assigned ? "info" : "ready"} label={t(assigned ? "proxies.inUse" : "proxies.free")} />
            <div className="row-actions"><IconButton label={t("proxies.manageAccounts")} icon={<UsersRound aria-hidden />} disabled={Boolean(busy)} onClick={() => setManagedProxyId(entry.id)} /><IconButton label={t("common.delete")} icon={<Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={() => void remove([entry.id])} /></div>
          </div>;
        })}</div>}
    {managedProxy ? <ProxyAccountsDialog entry={managedProxy} accounts={accountList} onSaved={setPool} onClose={() => setManagedProxyId(null)} /> : null}
  </div>;
}

function ProxyAccountsDialog({ entry, accounts, onSaved, onClose }: { entry: ProxyPoolEntry; accounts: AccountSummary[]; onSaved: (pool: ProxyPoolSummary) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [selected, setSelected] = useState(entry.assignedAccountIds);
  const [query, setQuery] = useState("");
  const visible = accounts.filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType));
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.includes(account.id));
  const save = async () => {
    const result: { current: StoredProxyAssignmentResult | null } = { current: null };
    const ok = await perform(`proxy-accounts-${entry.id}`, async () => { result.current = await relayCommands.setStoredProxyAccounts(entry.id, selected); }, "feedback.saved");
    if (ok && result.current) {
      onSaved(result.current.pool);
      onClose();
    }
  };
  return <Dialog wide title={t("proxies.manageAccountsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `proxy-accounts-${entry.id}`} onClick={() => void save()}>{t("common.save")}</Button></>}><div className="relay-form proxy-account-manager"><div className="proxy-manager-endpoint"><Network aria-hidden /><div><strong>{entry.endpoint}</strong><small>{t("proxies.assignedCount", { count: selected.length })}</small></div></div><div className="table-toolbar"><label className="toggle-row"><input type="checkbox" checked={allSelected} disabled={!accounts.length} onChange={(event) => setSelected(event.target.checked ? accounts.map((account) => account.id) : [])} /><span>{t("proxies.selectAll", { count: accounts.length })}</span></label><label className="search-field"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} /></label></div><div className="scope-grid proxy-account-grid">{visible.map((account) => <label key={account.id}><input type="checkbox" checked={selected.includes(account.id)} onChange={() => setSelected((current) => current.includes(account.id) ? current.filter((id) => id !== account.id) : [...current, account.id])} /><span className="proxy-account-identity" title={account.label}><strong>{account.label}</strong></span><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></label>)}</div>{!visible.length ? <NoResults /> : null}<p className="form-note">{t("proxies.sharedProxyHint")}</p></div></Dialog>;
}

function proxyLocationLabel(entry: ProxyPoolEntry, language: string, t: TFunction) {
  let country = entry.countryCode;
  if (country) {
    try {
      country = new Intl.DisplayNames([language], { type: "region" }).of(country) ?? country;
    } catch {
      // Keep the declared country code when the runtime cannot localize it.
    }
  }
  return [country, entry.region ? t("proxies.regionValue", { region: entry.region }) : null].filter(Boolean).join(" · ") || t("proxies.locationUnknown");
}

export function ProxyImportDialog({ onImported, onClose }: { onImported: () => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [result, setResult] = useState<{ added: number; duplicates: number } | null>(null);
  const proxyUrls = content.split(/\r?\n/).map((value) => value.trim()).filter(Boolean);
  const importProxies = async () => {
    let next: Awaited<ReturnType<typeof relayCommands.importProxyPool>> | null = null;
    const ok = await perform("proxy-import", async () => { next = await relayCommands.importProxyPool(proxyUrls); }, "feedback.saved");
    if (!ok || !next) return;
    setResult(next);
    setContent("");
    onImported();
  };
  return <Dialog wide title={t("proxies.importTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{result ? t("common.done") : t("common.cancel")}</Button><Button variant="primary" icon={<Upload aria-hidden />} busy={busy === "proxy-import"} disabled={!proxyUrls.length} onClick={() => void importProxies()}>{t("proxies.importCount", { count: proxyUrls.length })}</Button></>}><div className="relay-form proxy-import-form"><div className="proxy-import-intro"><Network aria-hidden /><div><strong>{t("proxies.importIntro")}</strong><p>{t("proxies.importHint")}</p></div></div><label className="relay-field"><span>{t("proxies.proxyList")}</span><div className="proxy-list-field"><textarea className={revealed ? "" : "secret-textarea"} value={content} onChange={(event) => { setContent(event.target.value); setResult(null); }} placeholder={t("proxies.proxyListPlaceholder")} autoComplete="off" spellCheck={false} /><IconButton type="button" label={revealed ? t("common.hide") : t("common.reveal")} icon={revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setRevealed((value) => !value)} /></div></label><div className="proxy-format-line"><span>{t("proxies.supportedFormats")}</span><code>host:port:user:pass</code><code>user:pass@host:port</code><code>http(s)://...</code></div>{result ? <p className="form-note success-text" role="status">{t("proxies.importResult", result)}</p> : <p className="form-note">{t("proxies.credentialsProtected")}</p>}</div></Dialog>;
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
  const current = pool?.entries.find((entry) => entry.assignedAccountIds.includes(account.id));
  const available = pool?.entries ?? [];
  useEffect(() => {
    if (!pool || initialized.current) return;
    initialized.current = true;
    if (current) {
      setChoice("stored");
      setProxyId(current.id);
    } else if (account.proxyMode === "account") {
      setChoice("custom");
    }
  }, [account.proxyMode, current, pool]);
  const apply = async () => {
    const result: { current: StoredProxyAssignmentResult | null } = { current: null };
    const ok = await perform(`proxy-${account.id}`, async () => {
      if (choice === "direct") await relayCommands.setAccountProxy(account.id, null, true);
      else if (choice === "common") await relayCommands.setAccountProxy(account.id, null);
      else if (choice === "automatic") result.current = await relayCommands.assignAutomaticProxies([account.id]);
      else if (choice === "stored") result.current = await relayCommands.assignStoredProxy(account.id, proxyId);
      else await relayCommands.setAccountProxy(account.id, proxyUrl.trim());
    }, "feedback.saved");
    if (!ok) return;
    if (result.current?.unavailable) {
      setUnavailable(true);
      return;
    }
    onClose();
  };
  const directBlocked = Boolean(runtime?.gateway.accountProxyRequired);
  const commonConfigured = Boolean(runtime?.gateway.commonProxyConfigured);
  const valid = Boolean(pool) && (choice !== "direct" || !directBlocked) && (choice !== "common" || commonConfigured) && (choice !== "stored" || proxyId) && (choice !== "custom" || proxyUrl.trim()) && (choice !== "automatic" || pool!.total > 0 || Boolean(current));
  const choose = (value: AccountProxyChoice) => { setChoice(value); setUnavailable(false); };
  return <Dialog title={t("proxies.accountTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!valid} onClick={() => void apply()}>{t("common.save")}</Button></>}><div className="relay-form proxy-route-form">{!pool ? <div className="center-loading"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div> : <>
    <div className="proxy-route-options" role="radiogroup" aria-label={t("proxies.accountRoute")}>
      <ProxyRouteOption value="direct" selected={choice === "direct"} disabled={directBlocked} icon={<WifiOff aria-hidden />} label={t("proxies.direct")} hint={t(directBlocked ? "proxies.directBlockedHint" : "proxies.directHint")} onSelect={choose} />
      <ProxyRouteOption value="automatic" selected={choice === "automatic"} disabled={!pool.total && !current} icon={<Shuffle aria-hidden />} label={t("proxies.assignAutomatically")} hint={t("proxies.storedAvailable", { count: pool.total })} onSelect={choose} />
      <ProxyRouteOption value="stored" selected={choice === "stored"} disabled={!available.length} icon={<Database aria-hidden />} label={t("proxies.chooseStored")} hint={t("proxies.chooseStoredShortHint")} onSelect={(value) => { choose(value); setProxyId((currentId) => currentId || available[0]?.id || ""); }} />
      <ProxyRouteOption value="custom" selected={choice === "custom"} icon={<Plus aria-hidden />} label={t("proxies.addCustom")} hint={t("proxies.addCustomShortHint")} onSelect={choose} />
      {commonConfigured ? <ProxyRouteOption value="common" selected={choice === "common"} icon={<Network aria-hidden />} label={t("proxies.useCommon")} hint={t("proxies.useCommonHint")} onSelect={choose} /> : null}
    </div>
    {choice === "stored" && available.length ? <div className="proxy-route-control"><OptionMenu className="field-option-menu" label={t("proxies.chooseStored")} value={proxyId || available[0].id} onChange={setProxyId} options={available.map((entry) => ({ value: entry.id, label: entry.endpoint }))} /></div> : null}
    {choice === "custom" ? <div className="proxy-route-control"><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} /></div> : null}
  </>}{unavailable ? <p role="alert" className="form-note error-text">{t("proxies.noStoredProxy")}</p> : null}</div></Dialog>;
}

function RemoteAccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform, runtime } = useRelayState();
  const [choice, setChoice] = useState<AccountProxyChoice>(() => account.proxyMode === "common" ? "common" : account.proxyMode === "account" ? "custom" : "direct");
  const [proxyUrl, setProxyUrl] = useState("");
  const commonConfigured = Boolean(runtime?.gateway.commonProxyConfigured);
  const directBlocked = Boolean(runtime?.gateway.accountProxyRequired);
  const apply = async () => {
    const ok = await perform(`proxy-${account.id}`, () => relayCommands.remoteAction({ type: "set_account_proxy", id: account.id }, { proxyUrl: choice === "custom" ? proxyUrl.trim() : null, bypassCommonProxy: choice === "direct" }), "feedback.saved");
    if (ok) onClose();
  };
  const valid = (choice !== "direct" || !directBlocked) && (choice !== "common" || commonConfigured) && (choice !== "custom" || Boolean(proxyUrl.trim()));
  return <Dialog title={t("proxies.accountTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!valid} onClick={() => void apply()}>{t("common.save")}</Button></>}><div className="relay-form proxy-route-form">
    <div className="proxy-route-options" role="radiogroup" aria-label={t("proxies.accountRoute")}>
      <ProxyRouteOption value="direct" selected={choice === "direct"} disabled={directBlocked} icon={<WifiOff aria-hidden />} label={t("proxies.direct")} hint={t(directBlocked ? "proxies.directBlockedHint" : "proxies.directHint")} onSelect={setChoice} />
      <ProxyRouteOption value="custom" selected={choice === "custom"} icon={<Plus aria-hidden />} label={t("proxies.addCustom")} hint={t("proxies.addCustomShortHint")} onSelect={setChoice} />
      {commonConfigured ? <ProxyRouteOption value="common" selected={choice === "common"} icon={<Network aria-hidden />} label={t("proxies.useCommon")} hint={t("proxies.useCommonHint")} onSelect={setChoice} /> : null}
    </div>
    {choice === "custom" ? <div className="proxy-route-control"><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} /><p className="form-note">{t("proxies.savedHidden")}</p></div> : null}
  </div></Dialog>;
}

function ProxyRouteOption({ value, selected, disabled = false, icon, label, hint, onSelect }: { value: AccountProxyChoice; selected: boolean; disabled?: boolean; icon: ReactNode; label: string; hint: string; onSelect: (value: AccountProxyChoice) => void }) {
  return <button type="button" role="radio" aria-checked={selected} disabled={disabled} className={selected ? "selected" : ""} onClick={() => onSelect(value)}>{icon}<span><strong>{label}</strong><small>{hint}</small></span></button>;
}

export function BulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { mode } = useRelayState();
  return mode === "local" ? <LocalBulkProxyDialog accountIds={accountIds} onClose={onClose} /> : <RemoteBulkProxyDialog accountIds={accountIds} onClose={onClose} />;
}

function LocalBulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool, setPool } = useProxyPool();
  const [result, setResult] = useState<StoredProxyAssignmentResult | null>(null);
  const accounts = (runtime?.accounts ?? []).filter((account) => accountIds.includes(account.id));
  const needProxy = accounts.filter((account) => account.proxyMode !== "account").length;
  const assign = async () => {
    const next: { current: StoredProxyAssignmentResult | null } = { current: null };
    const ok = await perform("proxy-bulk", async () => { next.current = await relayCommands.assignAutomaticProxies(accounts.map((account) => account.id)); }, "feedback.saved");
    if (ok && next.current) {
      setResult(next.current);
      setPool(next.current.pool);
    }
  };
  return <Dialog title={t("proxies.bulkTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{result ? t("common.done") : t("common.cancel")}</Button><Button variant="primary" busy={busy === "proxy-bulk"} disabled={!pool || !accounts.length || (needProxy > 0 && pool.total === 0)} onClick={() => void assign()}>{t("proxies.assignAutomatically")}</Button></>}><div className="relay-form"><div className="proxy-assignment-summary"><div><span>{t("connections.accounts")}</span><strong>{accounts.length}</strong></div><div><span>{t("proxies.needProxy")}</span><strong>{needProxy}</strong></div><div><span>{t("proxies.total")}</span><strong>{pool?.total ?? "-"}</strong></div></div><p className="form-note">{t("proxies.bulkStoredHint")}</p>{pool && needProxy > 0 && pool.total === 0 ? <p className="form-note warning-text">{t("proxies.noStored")}</p> : null}{result ? <p role="status" className="form-note success-text">{t("proxies.bulkStoredResult", result)}</p> : null}</div></Dialog>;
}

function RemoteBulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const accountById = new Map((runtime?.accounts ?? []).map((account) => [account.id, account]));
  const accounts = accountIds.map((accountId) => accountById.get(accountId)).filter((account): account is AccountSummary => Boolean(account));
  const [selected, setSelected] = useState(() => accounts.map((account) => account.id));
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [result, setResult] = useState<ProxyAssignmentResult | null>(null);
  const proxyUrls = content.split(/\r?\n/).map((value) => value.trim()).filter(Boolean);
  const selectedAccountIds = accounts.filter((account) => selected.includes(account.id)).map((account) => account.id);
  const valid = selectedAccountIds.length > 0 && proxyUrls.length >= selectedAccountIds.length;
  const toggle = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const assign = async () => {
    let response: ProxyAssignmentResult | null = null;
    const ok = await perform("proxy-bulk", async () => {
      response = await relayCommands.remoteAction({ type: "assign_account_proxies" }, { accountIds: selectedAccountIds, proxyUrls }) as ProxyAssignmentResult;
    }, "feedback.saved");
    if (ok) {
      setResult(response);
      setContent("");
    }
  };
  return <Dialog wide title={t("proxies.bulkTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button><Button variant="primary" busy={busy === "proxy-bulk"} disabled={!valid} onClick={assign}>{t("proxies.assign")}</Button></>}><div className="relay-form"><label className="toggle-row"><input type="checkbox" checked={selectedAccountIds.length === accounts.length && accounts.length > 0} onChange={(event) => setSelected(event.target.checked ? accounts.map((account) => account.id) : [])} /><span>{t("proxies.selectAll", { count: accounts.length })}</span></label><fieldset><legend>{t("connections.accounts")}</legend><div className="scope-grid proxy-account-grid">{accounts.map((account) => <label key={account.id}><input type="checkbox" checked={selected.includes(account.id)} onChange={() => toggle(account.id)} />{account.label}</label>)}</div></fieldset><label className="relay-field"><span>{t("proxies.proxyList")}</span><div className="proxy-list-field"><textarea className={revealed ? "" : "secret-textarea"} value={content} onChange={(event) => { setContent(event.target.value); setResult(null); }} placeholder={t("proxies.proxyListPlaceholder")} autoComplete="off" spellCheck={false} /><IconButton type="button" label={revealed ? t("common.hide") : t("common.reveal")} icon={revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setRevealed((value) => !value)} /></div></label><p className="form-note">{t("proxies.bulkHint", { selected: selectedAccountIds.length, provided: proxyUrls.length })}</p>{result ? <p role="status" className="form-note success-text">{t("proxies.bulkResult", result)}</p> : null}</div></Dialog>;
}
