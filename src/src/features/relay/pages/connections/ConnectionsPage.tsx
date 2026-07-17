import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { CalendarDays, Check, Clock3, Copy, CreditCard, Download, ExternalLink, Eye, EyeOff, Layers3, ListMinus, ListPlus, Loader2, LogIn, Network, Pencil, Play, Plus, Power, RefreshCw, ShieldAlert, Trash2, Unlink, Upload, UserRoundX, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createSavedTopUpIntentAndOpen, prepareTopUpAmount, resetKey, saveKey } from "../../../../tauri";
import { defaultWakeInput, relayCommands } from "../../api/commands";
import type { AccountExportFormat, AccountSummary, ConfirmAccountImportResponse, ImportSession, OAuthFlow, ProxyAssignmentResult, ProxyPoolSummary, RelayMode, RuntimeSnapshot, SourceSummary, StoredProxyAssignmentResult, WakeTask } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import {
  Button,
  AccountPlanBadge,
  ActionMenu,
  ActionMenuItem,
  Dialog,
  EmptyState,
  IconButton,
  OptionMenu,
  PageHeader,
  QuotaStack,
  SecretField,
  StatusBadge,
  Tabs,
  copyText,
  accountPlanOption,
  apiSourcePriority,
  apiSourceRole,
  compareAccountPlans,
  useConfirm,
} from "../../components/Ui";
import type { ApiSourceRole } from "../../components/Ui";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useRelayState } from "../../state/RelayStateProvider";
import { formatTokenSpeed, latestLocalAccountSpeeds } from "../../usageSpeed";

type View = "sources" | "accounts" | "proxies" | "automations" | "remote" | "api";
type DialogKind = "source" | "automation" | "remote" | "deploy" | "ready" | "topup" | "accountProxy" | "bulkProxies" | "proxyImport" | "oauthSetup" | "accountExport" | null;
type ParticipationFilter = "all" | "included" | "excluded";
type ImportFailure = { itemId: string; code: string; label?: string; identity?: string };

const accountExportFormats: Array<{ value: AccountExportFormat; label: string }> = [
  { value: "sub2api", label: "sub2api" },
  { value: "cpa", label: "CPA" },
  { value: "cockpit", label: "Cockpit" },
  { value: "9router", label: "9router" },
  { value: "codex", label: "ChatGPT" },
  { value: "axon_hub", label: "AxonHub" },
  { value: "codex_manager", label: "Codex-Manager" },
];

export function ConnectionsPage({ onImport }: { onImport: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, readyState, busy, perform, refresh } = useRelayState();
  const [view, setView] = useState<View>(mode === "zenith" ? "api" : "accounts");
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [query, setQuery] = useState("");
  const [editingSource, setEditingSource] = useState<SourceSummary | null>(null);
  const [editingAutomation, setEditingAutomation] = useState<WakeTask | null>(null);
  const [proxyAccount, setProxyAccount] = useState<AccountSummary | null>(null);
  const [bulkProxyAccountIds, setBulkProxyAccountIds] = useState<string[]>([]);
  const [exportAccountIds, setExportAccountIds] = useState<string[]>([]);
  const [oauthAccountId, setOauthAccountId] = useState<string | null>(null);
  const [proxyRevision, setProxyRevision] = useState(0);
  const oauth = useOAuthSignIn((result) => {
    setOauthAccountId(result.account.id);
    setDialog("oauthSetup");
  });
  const remoteFeatures = new Set(runtime?.capabilities.features ?? []);
  const supports = (feature: string) => mode !== "remote" || remoteFeatures.has(feature);
  const canImportAccounts = mode !== "remote" || supports("account_batch_import");
  const canManageProxies = supports("account_proxies");
  const canExportAccounts = supports("account_export");
  const canRevealAccountIdentity = mode === "local" || supports("account_identity_reveal");
  const showTableToolbar = view === "sources"
    ? Boolean(runtime?.sources.length)
    : view === "automations" && Boolean(runtime?.automations.length);

  useEffect(() => {
    setView(mode === "zenith" ? "api" : "accounts");
    setDialog(null);
    setEditingSource(null);
    setEditingAutomation(null);
    setProxyAccount(null);
    setBulkProxyAccountIds([]);
    setExportAccountIds([]);
    setOauthAccountId(null);
  }, [mode]);

  useEffect(() => setQuery(""), [mode, view]);

  useEffect(() => {
    if (mode !== "remote") return;
    if (!runtime) { if (view !== "remote") setView("remote"); return; }
    if ((view === "accounts" && !supports("accounts")) || (view === "sources" && !supports("sources")) || (view === "automations" && !supports("wake_tasks"))) setView("remote");
  }, [mode, runtime, view]);

  const tabs = mode === "zenith"
    ? [{ id: "api", label: t("connections.api") }]
    : [
        ...(supports("accounts") ? [{ id: "accounts", label: t("connections.accounts") }] : []),
        ...(mode === "local" ? [{ id: "proxies", label: t("proxies.storage") }] : []),
        ...(supports("sources") ? [{ id: "sources", label: t("connections.sources") }] : []),
        ...(supports("wake_tasks") ? [{ id: "automations", label: t("connections.automations") }] : []),
        ...(mode === "remote" ? [{ id: "remote", label: t("connections.remoteServer") }] : []),
      ];

  const primaryLabel = view === "accounts"
    ? mode === "local" ? t("accounts.signIn") : t("connections.import")
    : view === "proxies"
      ? t("proxies.import")
    : view === "sources"
      ? t("sources.add")
      : view === "automations"
        ? t("automations.add")
        : view === "remote"
          ? runtime ? t("remote.refresh") : t("remote.connect")
          : readyState?.providerActive ? t("readyApi.topUp") : t("readyApi.connect");

  const primaryAction = () => {
    if (view === "accounts" && !canImportAccounts) return;
    if (view === "accounts" && mode === "local") {
      void oauth.start();
      return;
    }
    if (view === "accounts" && mode === "remote") {
      onImport();
      return;
    }
    if (view === "proxies") {
      setDialog("proxyImport");
      return;
    }
    if (view === "remote" && runtime) {
      void perform("remote-refresh", relayCommands.refreshRemoteCapabilities, "feedback.refreshed");
      return;
    }
    if (view === "api" && readyState?.providerActive) {
      setDialog("topup");
      return;
    }
    setEditingSource(null);
    setEditingAutomation(null);
    setDialog(
      view === "sources" ? "source"
          : view === "automations" ? "automation"
            : view === "remote" ? "remote"
              : "ready",
    );
  };

  return (
    <section className="relay-page" data-view={view}>
      <PageHeader
        title={t("nav.connections")}
        subtitle={t(`connections.subtitles.${mode}`)}
        actions={
          <>
            {view === "accounts" && mode === "local" ? (
              <Button variant="secondary" icon={<Upload aria-hidden />} disabled={!canImportAccounts} title={!canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>
                {t("connections.import")}
              </Button>
            ) : null}
            <Button variant="primary" icon={view === "accounts" ? mode === "local" ? <LogIn aria-hidden /> : <Upload aria-hidden /> : view === "proxies" ? <Upload aria-hidden /> : view === "remote" && runtime ? <RefreshCw aria-hidden /> : <Plus aria-hidden />} busy={view === "accounts" && mode === "local" && busy === "oauth-start"} disabled={view === "accounts" && !canImportAccounts} title={view === "accounts" && !canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={primaryAction}>
              {primaryLabel}
            </Button>
          </>
        }
      />
      <Tabs value={view} items={tabs} onChange={(id) => setView(id as View)} label={t("connections.views")} />
      {showTableToolbar ? <div className="table-toolbar">
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
        {view === "automations" && mode === "local" ? <Button variant="secondary" icon={<Play aria-hidden />} busy={busy === "wake-due"} onClick={() => perform("wake-due", relayCommands.runWakeConfirmations, "feedback.checked")}>{t("automations.runDue")}</Button> : null}
        <Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={refresh}>{t("common.refresh")}</Button>
      </div> : null}

      {view === "sources" ? <SourcesTable query={query} onEdit={(source) => { setEditingSource(source); setDialog("source"); }} /> : null}
      {view === "accounts" ? <AccountsTable query={query} onQuery={setQuery} canImport={canImportAccounts} canManageProxies={canManageProxies} canExport={canExportAccounts} canRevealIdentity={canRevealAccountIdentity} onImport={onImport} onSignIn={() => void oauth.start()} onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }} onBulkProxies={(accountIds) => { setBulkProxyAccountIds(accountIds); setDialog("bulkProxies"); }} onExport={(accountIds) => { setExportAccountIds(accountIds); setDialog("accountExport"); }} /> : null}
      {view === "proxies" ? <ProxyStorageView revision={proxyRevision} onImport={() => setDialog("proxyImport")} /> : null}
      {view === "automations" ? <AutomationsTable query={query} onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}
      {view === "remote" ? <RemoteView onConnect={() => setDialog("remote")} onDeploy={() => setDialog("deploy")} /> : null}
      {view === "api" ? <ReadyApiView connected={Boolean(readyState?.providerActive)} onConnect={() => setDialog("ready")} onTopUp={() => setDialog("topup")} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {oauth.flow ? <OAuthDialog flow={oauth.flow} onCancel={oauth.cancel} /> : null}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "remote" ? <RemoteDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "deploy" ? <DeployDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "ready" ? <ReadyApiDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "topup" ? <TopUpDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "accountProxy" && proxyAccount ? <AccountProxyDialog account={proxyAccount} onClose={() => { setDialog(null); setProxyAccount(null); }} /> : null}
      {dialog === "bulkProxies" ? <BulkProxyDialog accountIds={bulkProxyAccountIds} onClose={() => setDialog(null)} /> : null}
      {dialog === "proxyImport" ? <ProxyImportDialog onImported={() => setProxyRevision((value) => value + 1)} onClose={() => setDialog(null)} /> : null}
      {dialog === "oauthSetup" && oauthAccountId ? <OAuthAccountSetupDialog accountId={oauthAccountId} onClose={() => { setDialog(null); setOauthAccountId(null); }} /> : null}
      {dialog === "accountExport" ? <AccountExportDialog accountIds={exportAccountIds} onClose={() => { setDialog(null); setExportAccountIds([]); }} /> : null}
      {busy ? <span className="sr-only" aria-live="polite">{t("common.working")}</span> : null}
    </section>
  );
}

function SourcesTable({ query, onEdit }: { query: string; onEdit: (source: SourceSummary) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const canTest = mode !== "remote" || runtime?.capabilities.features.includes("diagnostics");
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(query, source.name, source.baseUrl, source.wireApi, source.models));
  if (!sources.length) return <NoResults />;
  const updateParticipation = (source: SourceSummary, inPool: boolean) => perform(
    `source-pool-${source.id}`,
    () => mode === "local"
      ? relayCommands.setPoolMembership([], [source.id], inPool)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [source.id], inPool }),
    "feedback.saved",
  );
  return (
    <div className="relay-table-wrap">
      <table className="relay-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.protocol")}</th><th>{t("common.models")}</th><th>{t("sources.poolMembership")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => (
          <tr key={source.id}>
            <td><StatusBadge status={source.enabled && source.secretAvailable ? "ready" : "disabled"} label={source.enabled ? t("common.enabled") : t("common.disabled")} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td>{source.wireApi === "chat_completions" ? "Chat Completions" : "Responses"}</td>
            <td>{source.models.length}</td>
            <td><div className="source-pool-membership"><strong>{t(`sources.roles.${apiSourceRole(source.priority)}`)}</strong><label><input type="checkbox" checked={source.inPool} disabled={busy === `source-pool-${source.id}`} aria-label={t(source.inPool ? "sources.removeFromPool" : "sources.includeInPool", { name: source.name })} onChange={(event) => void updateParticipation(source, event.target.checked)} /><span>{t(source.inPool ? "sources.inPool" : "sources.outOfPool")}</span></label></div></td>
            <td className="row-actions">
              <IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={!canTest || busy === `test-${source.id}`} title={!canTest ? t("remote.capabilityUnavailable") : t("common.test")} onClick={() => perform(`test-${source.id}`, () => mode === "local" ? relayCommands.testSource(source.id) : relayCommands.remoteAction({ type: "test_source", id: source.id }), "feedback.checked")} />
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <ActionMenu>
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => mode === "local" ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")}>{source.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("sources.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${source.id}`, () => mode === "local" ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
            </td>
          </tr>
        ))}</tbody>
      </table>
    </div>
  );
}

function AccountsTable({ query, onQuery, canImport, canManageProxies, canExport, canRevealIdentity, onImport, onSignIn, onProxy, onBulkProxies, onExport }: { query: string; onQuery: (value: string) => void; canImport: boolean; canManageProxies: boolean; canExport: boolean; canRevealIdentity: boolean; onImport: () => void; onSignIn: () => void; onProxy: (account: AccountSummary) => void; onBulkProxies: (accountIds: string[]) => void; onExport: (accountIds: string[]) => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, localUsage, perform, activateCodexProfile, refresh, busy } = useRelayState();
  const confirm = useConfirm();
  const [selected, setSelected] = useState<string[]>([]);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [revealedIdentities, setRevealedIdentities] = useState<Record<string, string>>({});
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const allAccounts = runtime?.accounts ?? [];
  const accountSpeeds = useMemo(() => mode === "local" ? latestLocalAccountSpeeds(localUsage) : new Map<string, number>(), [localUsage, mode]);
  const [nowMs, setNowMs] = useState(Date.now());
  useEffect(() => {
    const expirations = allAccounts.map((account) => account.subscription.activeUntilMs).filter((value): value is number => value != null);
    if (!expirations.length) return;
    const urgent = expirations.some((value) => value > nowMs && value - nowMs < 24 * 60 * 60_000);
    const timer = window.setTimeout(() => setNowMs(Date.now()), urgent ? 1_000 : 60_000);
    return () => window.clearTimeout(timer);
  }, [allAccounts, nowMs]);
  const planOptions = new Map<string, { id: string; label: string; count: number }>();
  for (const account of allAccounts) {
    const option = accountPlanOption(account.subscription.planType, t("common.unknown"));
    const current = planOptions.get(option.id);
    planOptions.set(option.id, { ...option, count: (current?.count ?? 0) + 1 });
  }
  const plans = [...planOptions.values()].sort(compareAccountPlans);
  const errorCount = allAccounts.filter((account) => accountErrorCode(account)).length;
  const activePlan = planFilter === "all" || planOptions.has(planFilter) || (planFilter === "errors" && errorCount > 0) ? planFilter : "all";
  useEffect(() => setSelected((current) => current.filter((id) => allAccounts.some((account) => account.id === id))), [runtime?.accounts]);
  useEffect(() => { setSelected([]); setRevealedIdentities({}); setPlanFilter("all"); setParticipationFilter("all"); }, [mode]);
  if (!runtime?.accounts.length) {
    return <EmptyState title={t("accounts.emptyTitle")} description={t("accounts.emptyDescription")} action={<div className="inline-actions">{mode === "local" ? <Button variant="primary" onClick={onSignIn}>{t("accounts.signIn")}</Button> : null}<Button variant={mode === "local" ? "secondary" : "primary"} disabled={!canImport} title={!canImport ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>{t("accounts.import")}</Button></div>} />;
  }
  const accounts = [...runtime.accounts]
    .filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType, account.models))
    .filter((account) => activePlan === "all" || (activePlan === "errors" ? Boolean(accountErrorCode(account)) : accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan))
    .filter((account) => participationFilter === "all" || (participationFilter === "included") === accountParticipates(account))
    .sort(compareAccounts);
  const filtersActive = Boolean(query.trim()) || activePlan !== "all" || participationFilter !== "all";
  const filtersHideAccounts = filtersActive && accounts.length !== allAccounts.length;
  const selectedAccounts = accounts.filter((account) => selected.includes(account.id));
  const selectedIds = selectedAccounts.map((account) => account.id);
  const selectedCount = selectedAccounts.length;
  const exportIds = selectedCount ? selectedIds : allAccounts.map((account) => account.id);
  const canIncludeSelected = selectedAccounts.some((account) => !accountParticipates(account));
  const canExcludeSelected = selectedAccounts.some(accountParticipates);
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.includes(account.id));
  const toggleSelected = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const toggleAllVisible = (checked: boolean) => setSelected(checked ? accounts.map((account) => account.id) : []);
  const updateParticipation = async (account: AccountSummary, participate: boolean) => {
    if (mode === "local") {
      await relayCommands.setPoolMembership([account.id], [], participate);
      return;
    }
    await relayCommands.remoteAction(
      { type: "set_pool_membership" },
      { accountIds: [account.id], sourceIds: [], inPool: participate },
    );
  };
  const updateSelectedParticipation = async (participate: boolean) => {
    const ok = await perform("pool-membership-bulk", async () => {
      const accountIds = selectedAccounts.map((account) => account.id);
      if (mode === "local") await relayCommands.setPoolMembership(accountIds, [], participate);
      else await relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds, sourceIds: [], inPool: participate });
    }, "feedback.saved");
    if (ok) setSelected([]);
  };
  const deleteAccounts = async (accountIds: string[], operation: string) => {
    const ok = await perform(operation, async () => {
      for (const accountId of accountIds) {
        if (mode === "local") await relayCommands.deleteAccount(accountId);
        else await relayCommands.remoteAction({ type: "delete_account", id: accountId });
      }
    }, "feedback.deleted");
    if (!ok) await refresh().catch(() => undefined);
    if (ok) setSelected((current) => current.filter((id) => !accountIds.includes(id)));
    return ok;
  };
  const deleteSelected = async () => {
    const accountIds = selectedAccounts.map((account) => account.id);
    if (accountIds.length && await confirm(t("accounts.deleteSelectedConfirm", { count: accountIds.length }), { danger: true })) {
      await deleteAccounts(accountIds, "delete-selected-accounts");
    }
  };
  const refreshAndDeleteNonWorking = async () => {
    const refreshedSnapshot: { current: RuntimeSnapshot | null } = { current: null };
    const refreshed = await perform("refresh-non-working-accounts", async () => {
      await relayCommands.refreshAllAccountQuotas();
      refreshedSnapshot.current = await relayCommands.localState();
    }, "feedback.refreshed");
    if (!refreshed || !refreshedSnapshot.current) return;
    const accountIds = refreshedSnapshot.current.accounts.filter(accountIsTerminallyUnusable).map((account) => account.id);
    if (accountIds.length && await confirm(t("accounts.deleteNonWorkingConfirm", { count: accountIds.length }), { danger: true })) {
      await deleteAccounts(accountIds, "delete-non-working-accounts");
    }
  };
  const toggleIdentity = async (account: AccountSummary) => {
    if (revealedIdentities[account.id]) {
      setRevealedIdentities((current) => Object.fromEntries(Object.entries(current).filter(([id]) => id !== account.id)));
      return;
    }
    let identity = "";
    const ok = await perform(`identity-${account.id}`, async () => {
      const result = mode === "local"
        ? await relayCommands.revealLocalAccountIdentity(account.id)
        : await relayCommands.revealRemoteAccountIdentity(account.id);
      identity = result.identity;
    });
    if (ok && identity) setRevealedIdentities((current) => ({ ...current, [account.id]: identity }));
  };
  return (
    <>
    <div className="account-command-bar">
      <div className="account-command-context">
        <input type="checkbox" aria-label={t("accounts.selectAll")} title={t("accounts.selectAll")} checked={allSelected} disabled={!accounts.length} onChange={(event) => toggleAllVisible(event.target.checked)} />
        {selectedCount ? <span>{t("accounts.selectedCount", { count: selectedCount })}</span> : <label className="search-field account-search"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => onQuery(event.target.value)} placeholder={t("common.search")} /></label>}
      </div>
      <div className="account-command-actions">
        {selectedCount ? <>
          {canIncludeSelected ? <IconButton label={t("accounts.includeSelectedInPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListPlus aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(true)} /> : null}
          {canExcludeSelected ? <IconButton label={t("accounts.excludeSelectedFromPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(false)} /> : null}
          <IconButton label={t("accounts.exportSelected", { count: selectedCount })} icon={<Download aria-hidden />} disabled={!canExport || Boolean(busy)} title={!canExport ? t("remote.capabilityUnavailable") : t("accounts.exportSelected", { count: selectedCount })} onClick={() => onExport(exportIds)} />
          <IconButton className="danger" label={t("accounts.deleteSelected")} icon={busy === "delete-selected-accounts" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={deleteSelected} />
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} />
        </> : <>
          {mode === "local" ? <IconButton label={t("accounts.refreshAll")} icon={<RefreshCw className={busy === "quota-all" ? "spin" : undefined} aria-hidden />} disabled={busy === "quota-all"} onClick={() => perform("quota-all", relayCommands.refreshAllAccountQuotas, "feedback.refreshed")} /> : null}
          {mode === "local" ? <IconButton className="danger" label={t("accounts.deleteNonWorking")} icon={busy === "refresh-non-working-accounts" || busy === "delete-non-working-accounts" ? <Loader2 className="spin" aria-hidden /> : <UserRoundX aria-hidden />} disabled={Boolean(busy)} onClick={() => void refreshAndDeleteNonWorking()} /> : null}
          <ActionMenu className="account-row-menu account-bulk-menu">
            <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport} onClick={() => onExport(exportIds)}>{t("accounts.exportAll")}</ActionMenuItem>
            <ActionMenuItem icon={<Network aria-hidden />} disabled={!canManageProxies} onClick={() => onBulkProxies(accounts.map((account) => account.id))}>{t("proxies.assignBulk")}</ActionMenuItem>
          </ActionMenu>
        </>}
      </div>
    </div>
    <div className="account-filter-stack">
    <div className="account-plan-filters" role="group" aria-label={t("accounts.filterByParticipation")}>
      <span>{t("accounts.poolParticipation")}</span>
      {(["all", "included", "excluded"] as const).map((value) => {
        const count = value === "all" ? allAccounts.length : allAccounts.filter((account) => accountParticipates(account) === (value === "included")).length;
        return <button key={value} type="button" aria-pressed={participationFilter === value} aria-label={t("accounts.participationFilterOption", { state: t(`accounts.participation.${value}`), count })} onClick={() => { setSelected([]); setParticipationFilter(value); }}><span>{t(`accounts.participation.${value}`)}</span><small>{count}</small></button>;
      })}
    </div>
    {plans.length > 1 ? <div className="account-plan-filters" role="group" aria-label={t("accounts.filterByPlan")}>
      <span>{t("accounts.plan")}</span>
      <button type="button" aria-pressed={activePlan === "all"} aria-label={t("accounts.planFilterOption", { plan: t("accounts.allPlans"), count: allAccounts.length })} onClick={() => { setSelected([]); setPlanFilter("all"); }}><span>{t("accounts.allPlans")}</span><small>{allAccounts.length}</small></button>
      {errorCount ? <button type="button" className="error" aria-pressed={activePlan === "errors"} aria-label={t("accounts.planFilterOption", { plan: t("accounts.errorsOnly"), count: errorCount })} onClick={() => { setSelected([]); setPlanFilter("errors"); }}><ShieldAlert aria-hidden /><span>{t("accounts.errorsOnly")}</span><small>{errorCount}</small></button> : null}
      {plans.map((plan) => <button key={plan.id} type="button" aria-pressed={activePlan === plan.id} aria-label={t("accounts.planFilterOption", { plan: plan.label, count: plan.count })} onClick={() => { setSelected([]); setPlanFilter(plan.id); }}><span>{plan.label}</span><small>{plan.count}</small></button>)}
    </div> : null}
    </div>
    {filtersHideAccounts ? <div className="account-filter-summary" role="status" aria-live="polite"><span>{t("accounts.filterSummary", { visible: accounts.length, total: allAccounts.length })}</span><button type="button" onClick={() => { setSelected([]); onQuery(""); setPlanFilter("all"); setParticipationFilter("all"); }}><X aria-hidden /><span>{t("accounts.clearFilters")}</span></button></div> : null}
    {accounts.length ? <div className="account-list" role="list" aria-label={t("connections.accounts")} data-layout="list">
      {accounts.map((account) => {
        const errorCode = accountErrorCode(account);
        const participates = accountParticipates(account);
        const excludedByFreePolicy = account.routingExclusion === "free_plan_policy";
        const subscriptionEnded = account.subscription.activeUntilMs != null && account.subscription.activeUntilMs <= Date.now();
        const subscriptionEnd = subscriptionEndDisplay(account.subscription.activeUntilMs, i18n.resolvedLanguage ?? i18n.language, t, nowMs);
        const latestSpeed = accountSpeeds.get(account.id);
        return (
        <article key={account.id} className={`account-card${selected.includes(account.id) ? " selected" : ""}`} role="listitem">
          <div className="account-card-main">
            <input type="checkbox" aria-label={t("accounts.select", { name: account.label })} checked={selected.includes(account.id)} onChange={() => toggleSelected(account.id)} />
            <div className="account-identity">
              <strong className={revealedIdentities[account.id] ? "revealed" : undefined} title={revealedIdentities[account.id] ?? account.label}>{revealedIdentities[account.id] ?? account.label}</strong>
              <div>
                <span className={`account-health ${accountHealthTone(account.health)}`}>{t(`health.${account.health}`, { defaultValue: account.health })}</span>
                {latestSpeed != null ? <span className="account-token-speed" title={t("usage.latestSpeed")}>{formatTokenSpeed(latestSpeed, i18n.resolvedLanguage ?? i18n.language, t("usage.tokensPerSecondUnit"))}</span> : null}
              </div>
            </div>
            <div className="account-facts">
              <div className="account-fact account-fact-plan"><CreditCard className="account-fact-icon" aria-hidden /><span>{t("accounts.plan")}</span><strong><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></strong></div>
              <div className="account-fact account-fact-proxy"><Network className="account-fact-icon" aria-hidden /><span>{t("proxies.proxy")}</span><button type="button" className="proxy-status-button" disabled={!canManageProxies} title={!canManageProxies ? t("remote.capabilityUnavailable") : t("proxies.changeAccount")} onClick={() => onProxy(account)}><StatusBadge status={account.proxyAvailable === false ? "error" : account.proxyMode === "account" ? "info" : "ready"} label={account.proxyAvailable === false && account.proxyMode === "direct" ? t("proxies.modes.blocked") : t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></button></div>
              <div className="account-fact account-fact-pool"><Layers3 className="account-fact-icon" aria-hidden /><span>{t("accounts.poolParticipation")}</span><label className="account-pool-switch" title={excludedByFreePolicy ? t("accounts.participation.freePolicyHint") : participates ? t("accounts.excludeFromPool") : t("accounts.includeInPool")}><input type="checkbox" role="switch" checked={participates} disabled={busy === `pool-${account.id}`} aria-label={t("accounts.poolParticipationFor", { name: account.label })} onChange={(event) => void perform(`pool-${account.id}`, () => updateParticipation(account, event.target.checked), "feedback.saved")} /><strong>{excludedByFreePolicy && participates ? t("accounts.participation.freePolicy") : participates ? t("accounts.participation.included") : t("accounts.participation.excluded")}</strong></label></div>
            </div>
            <div className="account-row-action-list">
              <ActionMenu className="account-row-menu">
                <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport || !account.secretAvailable} onClick={() => onExport([account.id])}>{t("accounts.exportOne", { name: account.label })}</ActionMenuItem>
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void perform(`enable-${account.id}`, () => mode === "local" ? relayCommands.setAccountEnabled(account.id, !account.enabled) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled: !account.enabled }), "feedback.saved"); }}>{account.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("accounts.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${account.id}`, () => mode === "local" ? relayCommands.deleteAccount(account.id) : relayCommands.remoteAction({ type: "delete_account", id: account.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
              {canRevealIdentity ? <IconButton label={revealedIdentities[account.id] ? t("accounts.hideIdentity") : t("accounts.revealIdentity")} icon={revealedIdentities[account.id] ? <EyeOff aria-hidden /> : <Eye aria-hidden />} disabled={!account.secretAvailable || busy === `identity-${account.id}`} title={!account.secretAvailable ? t("accounts.credentialsUnavailable") : revealedIdentities[account.id] ? t("accounts.hideIdentity") : t("accounts.revealIdentity")} onClick={() => void toggleIdentity(account)} /> : null}
              <IconButton label={t("accounts.refreshQuota")} icon={<RefreshCw className={busy === `quota-${account.id}` ? "spin" : undefined} aria-hidden />} disabled={busy === `quota-${account.id}`} onClick={() => void perform(`quota-${account.id}`, () => mode === "local" ? relayCommands.refreshAccountQuota(account.id) : relayCommands.remoteAction({ type: "refresh_account", id: account.id }), "feedback.refreshed")} />
              {mode === "local" ? <IconButton label={t("accounts.launchAccount")} icon={<Play aria-hidden />} disabled={!account.secretAvailable || busy === `launch-account-${account.id}`} title={!account.secretAvailable ? t("accounts.credentialsUnavailable") : t("accounts.launchAccount")} onClick={() => void activateCodexProfile(`launch-account-${account.id}`, () => relayCommands.launchCodexAccount(account.id), true)} /> : null}
            </div>
          </div>
          <div className={`account-subscription-line${subscriptionEnded ? " expired" : ""}`} title={[subscriptionEnd.date, subscriptionEnd.relative].filter(Boolean).join(" · ")}><CalendarDays aria-hidden /><span>{subscriptionEnd.date}</span>{subscriptionEnd.relative ? <span className="account-subscription-countdown">{subscriptionEnd.relative}</span> : null}</div>
          {errorCode ? <button type="button" className="account-error-line" title={t("accounts.openErrorDetails", { code: errorCode })} aria-label={t("accounts.openErrorDetails", { code: errorCode })} onClick={() => setErrorDetails(account)}><ShieldAlert aria-hidden /><span>{accountErrorLabel(errorCode, t)}</span><code>{errorCode}</code></button> : null}
          <div className="account-card-quota"><QuotaStack snapshot={account.quota} /></div>
        </article>
        );
      })}
    </div> : <NoResults />}
    {errorDetails ? <AccountErrorDialog account={errorDetails} onClose={() => setErrorDetails(null)} /> : null}
    </>
  );
}

function AccountErrorDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const code = accountErrorCode(account) ?? "unknown";
  const authState = typeof account.authState === "string" ? account.authState : account.authState.state;
  const observedAtMs = account.quota.error?.observedAtMs ?? null;
  const details = JSON.stringify({
    code,
    message: accountErrorLabel(code, t),
    observed_at: observedAtMs ? new Date(observedAtMs).toISOString() : null,
    account: account.identityHint || account.label,
    health: account.health,
    auth_state: authState,
    subscription_status: account.subscription.status,
  }, null, 2);
  return <Dialog title={t("accounts.errorDetailsTitle")} onClose={onClose} footer={<><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => void copyText(details)}>{t("common.copy")}</Button><Button variant="primary" onClick={onClose}>{t("common.close")}</Button></>}><div className="config-preview account-error-json"><pre><code>{details}</code></pre></div><p className="form-note">{t("accounts.errorDetailsHint")}</p></Dialog>;
}

function AccountExportDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [format, setFormat] = useState<AccountExportFormat>("sub2api");
  const [confirmed, setConfirmed] = useState(false);
  const run = async (destination: "copy" | "download") => {
    const ok = await perform(`account-export-${destination}`, async () => {
      const input = { accountIds, format, destination } as const;
      const result = mode === "local"
        ? await relayCommands.exportLocalAccounts(input)
        : await relayCommands.exportRemoteAccounts(input);
      if (destination === "copy") {
        if (!result.content) throw new Error("account export content is missing");
        await copyText(result.content);
      }
    }, destination === "copy" ? "feedback.accountExportCopied" : "feedback.accountExportDownloaded");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.exportTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="secondary" icon={<Copy aria-hidden />} busy={busy === "account-export-copy"} disabled={!confirmed} onClick={() => run("copy")}>{t("accounts.copyExport")}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={busy === "account-export-download"} disabled={!confirmed} onClick={() => run("download")}>{t("accounts.downloadExport")}</Button></>}><div className="relay-form account-export-form"><div className="account-export-heading"><span>{t("accounts.exportFormat")}</span><strong>{t("accounts.exportCount", { count: accountIds.length })}</strong></div><div className="account-export-formats" role="radiogroup" aria-label={t("accounts.exportFormat")}>{accountExportFormats.map((option) => <button type="button" role="radio" aria-checked={format === option.value} key={option.value} onClick={() => setFormat(option.value)}><span className="account-export-radio" aria-hidden /><span>{option.label}</span></button>)}</div><div role="alert" className="account-export-warning"><ShieldAlert aria-hidden /><div><strong>{t("accounts.exportSensitiveTitle")}</strong><span>{t("accounts.exportWarning")}</span></div></div><label className="account-export-confirm"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} /><span>{t("accounts.confirmExport")}</span></label></div></Dialog>;
}

function useProxyPool(enabled = true, revision = 0) {
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

function ProxyStorageView({ revision, onImport }: { revision: number; onImport: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const { pool, setPool, failed, load } = useProxyPool(true, revision);
  const [query, setQuery] = useState("");
  const accounts = new Map((runtime?.accounts ?? []).map((account) => [account.id, account]));
  const entries = (pool?.entries ?? []).filter((entry) => matchesQuery(query, entry.endpoint, entry.assignedAccountId ? accounts.get(entry.assignedAccountId)?.label ?? entry.assignedAccountId : ""));
  const remove = async (proxyId: string) => {
    if (!await confirm(t("proxies.deleteConfirm"), { danger: true })) return;
    let next: ProxyPoolSummary | null = null;
    const ok = await perform(`proxy-delete-${proxyId}`, async () => { next = await relayCommands.deleteStoredProxy(proxyId); }, "feedback.saved");
    if (ok && next) setPool(next);
  };
  const release = async (accountId: string) => {
    const ok = await perform(`proxy-release-${accountId}`, () => relayCommands.setAccountProxy(accountId, null), "feedback.saved");
    if (ok) await load();
  };
  if (failed) return <EmptyState title={t("proxies.storageUnavailable")} description={t("proxies.storageUnavailableHint")} action={<Button variant="primary" icon={<RefreshCw aria-hidden />} onClick={() => void load()}>{t("common.retry")}</Button>} />;
  if (!pool) return <div className="center-loading" role="status"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div>;
  return <div className="proxy-storage">
    <div className="proxy-storage-summary" aria-label={t("proxies.storageSummary")}>
      <div><span>{t("proxies.total")}</span><strong>{pool.total}</strong></div>
      <div><span>{t("proxies.free")}</span><strong>{pool.free}</strong></div>
      <div><span>{t("proxies.assigned")}</span><strong>{pool.assigned}</strong></div>
    </div>
    {pool.total ? <div className="table-toolbar proxy-storage-toolbar"><label className="search-field"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("proxies.search")} /></label><Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={() => void load()}>{t("common.refresh")}</Button></div> : null}
    {!pool.total ? <EmptyState title={t("proxies.emptyTitle")} description={t("proxies.emptyDescription")} action={<Button variant="primary" icon={<Upload aria-hidden />} onClick={onImport}>{t("proxies.import")}</Button>} />
      : !entries.length ? <NoResults />
        : <div className="proxy-storage-list" role="list">{entries.map((entry) => {
          const account = entry.assignedAccountId ? accounts.get(entry.assignedAccountId) : null;
          return <div className="proxy-storage-row" role="listitem" key={entry.id}><Network aria-hidden /><div><strong>{entry.endpoint}</strong><small>{account?.label ?? (entry.assignedAccountId || t("proxies.readyForAssignment"))}</small></div><StatusBadge status={entry.assignedAccountId ? "info" : "ready"} label={t(entry.assignedAccountId ? "proxies.assigned" : "proxies.free")} />{entry.assignedAccountId ? <IconButton label={t("proxies.release")} icon={<Unlink aria-hidden />} disabled={busy === `proxy-release-${entry.assignedAccountId}`} onClick={() => void release(entry.assignedAccountId!)} /> : <IconButton label={t("common.delete")} icon={<Trash2 aria-hidden />} disabled={busy === `proxy-delete-${entry.id}`} onClick={() => void remove(entry.id)} />}</div>;
        })}</div>}
  </div>;
}

function ProxyImportDialog({ onImported, onClose }: { onImported: () => void; onClose: () => void }) {
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

function AccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { mode } = useRelayState();
  return mode === "local" ? <LocalAccountProxyDialog account={account} onClose={onClose} /> : <RemoteAccountProxyDialog account={account} onClose={onClose} />;
}

function LocalAccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const [choice, setChoice] = useState<"free" | "stored" | "custom" | "inherited">("inherited");
  const [proxyId, setProxyId] = useState("");
  const [proxyUrl, setProxyUrl] = useState("");
  const [unavailable, setUnavailable] = useState(false);
  const initialized = useRef(false);
  const current = pool?.entries.find((entry) => entry.assignedAccountId === account.id);
  const available = pool?.entries.filter((entry) => !entry.assignedAccountId || entry.assignedAccountId === account.id) ?? [];
  useEffect(() => {
    if (!pool || initialized.current) return;
    initialized.current = true;
    if (current) {
      setChoice("stored");
      setProxyId(current.id);
    } else if (pool.free > 0) {
      setChoice("free");
    }
  }, [current, pool]);
  const apply = async () => {
    const result: { current: StoredProxyAssignmentResult | null } = { current: null };
    const ok = await perform(`proxy-${account.id}`, async () => {
      if (choice === "inherited") await relayCommands.setAccountProxy(account.id, null);
      else if (choice === "free") result.current = await relayCommands.assignFreeProxies([account.id]);
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
  const valid = Boolean(pool) && (choice !== "stored" || proxyId) && (choice !== "custom" || proxyUrl.trim()) && (choice !== "free" || pool!.free > 0 || Boolean(current));
  return <Dialog title={t("proxies.accountTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!valid} onClick={() => void apply()}>{t("common.save")}</Button></>}><div className="relay-form"><div className="proxy-current"><span>{t("proxies.currentMode")}</span><StatusBadge status={account.proxyAvailable === false ? "error" : "ready"} label={t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></div>{!pool ? <div className="center-loading"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div> : <div className="proxy-choice-list" role="radiogroup" aria-label={t("proxies.accountRoute")}>
    <label className={choice === "free" ? "selected" : ""}><input type="radio" name="proxy-choice" checked={choice === "free"} disabled={pool.free === 0 && !current} onChange={() => { setChoice("free"); setUnavailable(false); }} /><span><strong>{t("proxies.assignAutomatically")}</strong><small>{t("proxies.freeAvailable", { count: pool.free })}</small></span></label>
    <label className={choice === "stored" ? "selected" : ""}><input type="radio" name="proxy-choice" checked={choice === "stored"} disabled={!available.length} onChange={() => { setChoice("stored"); setProxyId((value) => value || available[0]?.id || ""); setUnavailable(false); }} /><span><strong>{t("proxies.chooseStored")}</strong><small>{t("proxies.chooseStoredHint")}</small></span></label>
    {choice === "stored" && available.length ? <OptionMenu className="field-option-menu proxy-choice-control" label={t("proxies.chooseStored")} value={proxyId || available[0].id} onChange={setProxyId} options={available.map((entry) => ({ value: entry.id, label: entry.endpoint }))} /> : null}
    <label className={choice === "custom" ? "selected" : ""}><input type="radio" name="proxy-choice" checked={choice === "custom"} onChange={() => { setChoice("custom"); setUnavailable(false); }} /><span><strong>{t("proxies.addCustom")}</strong><small>{t("proxies.addCustomHint")}</small></span></label>
    {choice === "custom" ? <SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} /> : null}
    <label className={choice === "inherited" ? "selected" : ""}><input type="radio" name="proxy-choice" checked={choice === "inherited"} onChange={() => { setChoice("inherited"); setUnavailable(false); }} /><span><strong>{t("proxies.useInherited")}</strong><small>{t("proxies.useInheritedHint")}</small></span></label>
  </div>}{unavailable ? <p role="alert" className="form-note error-text">{t("proxies.noFreeProxy")}</p> : null}</div></Dialog>;
}

function RemoteAccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [proxyUrl, setProxyUrl] = useState("");
  const apply = async (value: string | null) => {
    const ok = await perform(`proxy-${account.id}`, () => relayCommands.remoteAction({ type: "set_account_proxy", id: account.id }, { proxyUrl: value }), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("proxies.accountTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>{account.proxyMode === "account" ? <Button variant="secondary" busy={busy === `proxy-${account.id}`} onClick={() => void apply(null)}>{t("proxies.useInherited")}</Button> : null}<Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!proxyUrl.trim()} onClick={() => void apply(proxyUrl.trim())}>{t("common.save")}</Button></>}><div className="relay-form"><div className="proxy-current"><span>{t("proxies.currentMode")}</span><StatusBadge status={account.proxyAvailable === false ? "error" : "ready"} label={t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></div><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder={t("proxies.proxyPlaceholder")} /><p className="form-note">{t("proxies.savedHidden")}</p></div></Dialog>;
}

function BulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
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
    const ok = await perform("proxy-bulk", async () => { next.current = await relayCommands.assignFreeProxies(accounts.map((account) => account.id)); }, "feedback.saved");
    if (ok && next.current) {
      setResult(next.current);
      setPool(next.current.pool);
    }
  };
  return <Dialog title={t("proxies.bulkTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{result ? t("common.done") : t("common.cancel")}</Button><Button variant="primary" busy={busy === "proxy-bulk"} disabled={!pool || !accounts.length || (needProxy > 0 && pool.free === 0)} onClick={() => void assign()}>{t("proxies.assignAutomatically")}</Button></>}><div className="relay-form"><div className="proxy-assignment-summary"><div><span>{t("connections.accounts")}</span><strong>{accounts.length}</strong></div><div><span>{t("proxies.needProxy")}</span><strong>{needProxy}</strong></div><div><span>{t("proxies.free")}</span><strong>{pool?.free ?? "-"}</strong></div></div><p className="form-note">{t("proxies.bulkStoredHint")}</p>{pool && pool.free < needProxy ? <p className="form-note warning-text">{t("proxies.notEnoughFree", { count: needProxy - pool.free })}</p> : null}{result ? <p role="status" className="form-note success-text">{t("proxies.bulkStoredResult", result)}</p> : null}</div></Dialog>;
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

function AutomationsTable({ query, onEdit }: { query: string; onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} />;
  }
  const automations = runtime.automations.filter((task) => matchesQuery(query, task.name, task.accountSelector.kind === "all_eligible" ? "" : task.accountSelector.values, task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : ""));
  if (!automations.length) return <NoResults />;
  return (
    <div className="relay-table-wrap">
        <table className="relay-table">
          <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("connections.accounts")}</th><th>{t("common.quota")}</th><th>{t("common.model")}</th><th>{t("automations.lastResult")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
          <tbody>{automations.map((task) => {
            const history = runtime.wakeHistory.filter((item) => item.taskId === task.id);
            const last = history[history.length - 1];
            return (
              <tr key={task.id}>
                <td><input type="checkbox" checked={task.enabled} aria-label={t("common.enabled")} onChange={() => perform(`automation-${task.id}`, () => mode === "local" ? relayCommands.setAutomationEnabled(task.id, !task.enabled) : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled: !task.enabled }), "feedback.saved")} /></td>
                <td><strong>{task.name}</strong></td>
                <td>{task.accountSelector.kind === "all_eligible" ? t("automations.allEligible") : task.accountSelector.kind === "account_ids" ? task.accountSelector.values.map((id) => runtime.accounts.find((account) => account.id === id)?.label ?? id).join(", ") : task.accountSelector.values.join(", ")}</td>
                <td>{task.windowKinds.map((item) => t(`quota.${item}`)).join(", ")}</td>
                <td>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</td>
                <td>{last ? t(`wake.${last.outcome}`, { defaultValue: last.outcome }) : t("common.never")}</td>
                <td className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem></ActionMenu></td>
              </tr>
            );
          })}</tbody>
        </table>
    </div>
  );
}

function RemoteView({ onConnect, onDeploy }: { onConnect: () => void; onDeploy: () => void }) {
  const { t } = useTranslation();
  const { runtime, perform } = useRelayState();
  const confirm = useConfirm();
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.gateway.running ? "ready" : "warning"} label={runtime.gateway.running ? t("common.online") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="danger" onClick={() => void confirm(t("remote.disconnectConfirm"), { danger: true }).then((accepted) => accepted && perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected"))}>{t("remote.disconnect")}</Button></div></section>;
}

function ReadyApiView({ connected, onConnect, onTopUp }: { connected: boolean; onConnect: () => void; onTopUp: () => void }) {
  const { t } = useTranslation();
  const { readyStats, perform } = useRelayState();
  const confirm = useConfirm();
  return <section className="ready-api-connection"><div className="recommended-line"><div><strong>Zenith API</strong><small>https://api.zenithmarket.dev/v1</small></div><span>{t("common.recommended")}</span></div><StatusBadge status={connected ? "ready" : "warning"} label={connected ? t("common.connected") : t("common.notConfigured")} /><p>{t("readyApi.connectionHint")}</p>{connected ? <><dl className="detail-list"><div><dt>{t("readyApi.balance")}</dt><dd>{readyStats?.balance ?? "-"}</dd></div><div><dt>{t("usage.requests")}</dt><dd>{readyStats?.requestsDisplay ?? readyStats?.requests ?? "-"}</dd></div></dl><div className="inline-actions"><Button variant="secondary" onClick={onTopUp}>{t("readyApi.topUp")}</Button><Button variant="secondary" onClick={onConnect}>{t("readyApi.updateKey")}</Button><Button variant="danger" onClick={() => void confirm(t("readyApi.disconnectConfirm"), { danger: true }).then((accepted) => accepted && perform("ready-disconnect", resetKey, "feedback.disconnected"))}>{t("remote.disconnect")}</Button></div></> : <Button variant="primary" onClick={onConnect}>{t("readyApi.connect")}</Button>}</section>;
}

export function SourceDialog({ source, onClose, addToPool = false }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [wireApi, setWireApi] = useState<SourceSummary["wireApi"]>(source?.wireApi ?? "responses");
  const [models, setModels] = useState(source?.models.join(", ") ?? "");
  const [allowed, setAllowed] = useState(source?.allowedModels.join(", ") ?? "");
  const [excluded, setExcluded] = useState(source?.excludedModels.join(", ") ?? "");
  const [role, setRole] = useState<ApiSourceRole>(apiSourceRole(source?.priority ?? 0));
  const [weight, setWeight] = useState(source?.weight ?? 100);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const base = { name, baseUrl, wireApi, models: parseList(models), allowedModels: parseList(allowed), excludedModels: parseList(excluded), draining: source?.draining ?? false, priority: apiSourcePriority(role), weight };
    const ok = await perform("source-save", async () => {
      if (!source) {
        const payload = { ...base, apiKey };
        const created = mode === "local"
          ? await relayCommands.createSource(payload) as { id: string }
          : await relayCommands.remoteAction({ type: "create_source" }, payload) as { id: string };
        if (addToPool) {
          if (mode === "local") await relayCommands.setPoolMembership([], [created.id], true);
          else await relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [created.id], inPool: true });
        }
        return;
      }
      if (mode === "local") {
        await relayCommands.updateSource({ sourceId: source.id, ...base });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...base, ...(apiKey ? { apiKey } : {}) });
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!source && !apiKey.trim()} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>}><form id="source-form" className="relay-form" onSubmit={submit}><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label><div className="relay-field"><span>{t("sources.protocol")}</span><OptionMenu className="field-option-menu" label={t("sources.protocol")} value={wireApi} onChange={(value) => setWireApi(value as SourceSummary["wireApi"])} options={[{ value: "responses", label: "Responses API" }, { value: "chat_completions", label: "Chat Completions" }]} /></div><SecretField label={source ? t("sources.replaceKey") : t("sources.apiKey")} value={apiKey} onChange={setApiKey} /><label className="relay-field"><span>{t("common.models")}</span><input value={models} onChange={(event) => setModels(event.target.value)} placeholder={t("sources.modelListPlaceholder")} /></label><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><div className="settings-row"><div className="relay-field"><span>{t("sources.poolRole")}</span><OptionMenu className="field-option-menu" label={t("sources.poolRole")} value={role} onChange={(value) => setRole(value as ApiSourceRole)} options={[{ value: "primary", label: t("sources.roles.primary") }, { value: "stabilizer", label: t("sources.roles.stabilizer") }, { value: "reserve", label: t("sources.roles.reserve") }]} /><small>{t(`sources.roleHints.${role}`)}</small></div><label><span>{t("pool.trafficShare")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label></div></form></Dialog>;
}

function OAuthDialog({ flow, onCancel }: { flow: OAuthFlow; onCancel: () => Promise<void> }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [now, setNow] = useState(Date.now);
  const [reopenAt, setReopenAt] = useState(() => Date.now() + 3_000);
  const [linkCopied, setLinkCopied] = useState(false);
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(interval);
  }, []);
  const secondsRemaining = Math.max(0, Math.ceil((flow.expiresAtMs - now) / 1_000));
  const reopenIn = Math.max(0, Math.ceil((reopenAt - now) / 1_000));
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const flowFailed = flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed";
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const reopen = async () => {
    const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId));
    if (opened) setReopenAt(Date.now() + 3_000);
  };
  const copyLink = async () => {
    await copyText(flow.authorizationUrl);
    setLinkCopied(true);
    window.setTimeout(() => setLinkCopied(false), 1_500);
  };
  return <Dialog
    title={t("accounts.signIn")}
    onClose={() => void onCancel()}
    footer={<Button variant="secondary" busy={busy === "oauth-cancel"} onClick={() => void onCancel()}>{t("common.cancel")}</Button>}
  >
    <div className="relay-form oauth-waiting">
      <div className="oauth-waiting-status"><Loader2 className="spin" aria-hidden /><div><strong>{t(callbackReceived ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong><p>{t("accounts.waitingForSignInHint")}</p></div></div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="oauth-expiry" role="timer"><Clock3 aria-hidden /><span>{t("accounts.oauthRemaining")}</span><strong>{formatCountdown(secondsRemaining)}</strong></div>
      <div className="oauth-link-actions">
        <Button variant="primary" icon={<ExternalLink aria-hidden />} busy={busy === "oauth-reopen"} disabled={flowUnavailable || reopenIn > 0} onClick={() => void reopen()}>{reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t("accounts.reopenSignIn")}</Button>
        <Button variant="secondary" icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />} disabled={flowUnavailable} onClick={() => void copyLink()}>{t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}</Button>
      </div>
    </div>
  </Dialog>;
}

function OAuthAccountSetupDialog({ accountId, onClose }: { accountId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const [addToPool, setAddToPool] = useState(true);
  const [assignProxy, setAssignProxy] = useState(false);
  const initialized = useRef(false);
  const account = runtime?.accounts.find((item) => item.id === accountId);
  useEffect(() => {
    if (!pool || initialized.current) return;
    initialized.current = true;
    setAssignProxy(pool.free > 0);
  }, [pool]);
  const apply = async () => {
    if (!addToPool && !assignProxy) {
      onClose();
      return;
    }
    const ok = await perform("oauth-setup", async () => {
      if (addToPool) await relayCommands.setPoolMembership([accountId], [], true);
      if (assignProxy) await relayCommands.assignFreeProxies([accountId]);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.accountAdded")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("accounts.configureLater")}</Button><Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}>{t("common.done")}</Button></>}><div className="relay-form oauth-account-setup"><div className="oauth-account-added"><Check aria-hidden /><div><strong>{account?.identityHint ?? t("accounts.accountReady")}</strong><p>{t("accounts.accountAddedHint")}</p></div></div><div className="post-import-options"><label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addAccountToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label><label><input type="checkbox" checked={assignProxy} disabled={!pool || pool.free === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignFreeAfterAdd")}</strong><small>{pool ? t(pool.free ? "proxies.freeAvailable" : "proxies.noFreeStored", { count: pool.free }) : t("common.loading")}</small></span></label></div></div></Dialog>;
}

function formatCountdown(seconds: number) {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}

export function ImportDialog({ initialPaths, modeOverride, defaultAddToPool = false, onImported, onClose }: { initialPaths?: string[]; modeOverride?: RelayMode; defaultAddToPool?: boolean; onImported?: () => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode: currentMode, runtime, perform, busy } = useRelayState();
  const mode = modeOverride ?? currentMode;
  const { pool: proxyPool } = useProxyPool(mode === "local");
  const [content, setContent] = useState("");
  const [session, setSession] = useState<ImportSession | null>(null);
  const [ownedSessionId, setOwnedSessionId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [commandFailed, setCommandFailed] = useState(false);
  const [completed, setCompleted] = useState<ImportFailure[] | null>(null);
  const [addToPool, setAddToPool] = useState(defaultAddToPool);
  const [assignProxy, setAssignProxy] = useState(false);
  const [fileLoading, setFileLoading] = useState(Boolean(initialPaths?.length));
  const activeSessionId = useRef<string | null>(null);
  const initialPreviewStarted = useRef(false);
  const proxyDefaultSet = useRef(false);
  const canImportToPool = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("account_import_to_pool"));
  useEffect(() => {
    if (!proxyPool || proxyDefaultSet.current) return;
    proxyDefaultSet.current = true;
    setAssignProxy(proxyPool.free > 0);
  }, [proxyPool]);
  const acceptSession = (next: ImportSession) => {
    setSession(next);
    setOwnedSessionId(next.sessionId);
    activeSessionId.current = next.sessionId;
    setCommandFailed(false);
    setCompleted(null);
    setSelected(next.preview.rows
      .filter((row) => row.selectable && row.defaultSelected)
      .map((row) => row.itemId));
  };
  const cancel = async () => {
    const sessionId = session?.sessionId ?? ownedSessionId;
    if (mode === "local" && sessionId && !completed) await perform("import-cancel", () => relayCommands.cancelImport(sessionId));
    activeSessionId.current = null;
    onClose();
  };
  const preview = async () => {
    if (mode === "local") {
      const result: { current: ImportSession | null } = { current: null };
      const ok = await perform("import-preview", async () => {
        const started = await relayCommands.startImport(content);
        setOwnedSessionId(started.sessionId);
        result.current = await relayCommands.prepareImport(started.sessionId, true);
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
  const confirm = async () => {
    if (!session) return;
    if (mode === "local") {
      const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
      const ok = await perform("import-confirm", async () => {
        result.current = await relayCommands.confirmImport(session.sessionId, selected, addToPool);
        if (!assignProxy) return;
        const accountIds = result.current.results.flatMap((item) => item.status === "succeeded" && item.account ? [item.account.account.id] : []);
        if (accountIds.length) await relayCommands.assignFreeProxies(accountIds);
      });
      if (!ok) {
        setSession(null);
        setSelected([]);
        setCommandFailed(true);
        activeSessionId.current = null;
        return;
      }
      const failures = collectImportFailures(result.current, session);
      if (result.current?.results.some((item) => item.status === "succeeded")) onImported?.();
      activeSessionId.current = null;
      if (failures.length) {
        setCompleted(failures);
        return;
      }
      onClose();
      return;
    }
    const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
    const ok = await perform("import-confirm", async () => {
      result.current = await relayCommands.remoteAction(
        { type: "confirm_account_batch_import" },
        { sessionId: session.sessionId, selectedItemIds: selected, probeMetadata: true, addToPool },
      ) as Awaited<ReturnType<typeof relayCommands.confirmImport>>;
    }, "feedback.accountAdded");
    if (!ok) {
      setCommandFailed(true);
      return;
    }
    const failures = collectImportFailures(result.current, session);
    if (result.current?.results.some((item) => item.status === "succeeded")) onImported?.();
    activeSessionId.current = null;
    if (failures.length) setCompleted(failures);
    else onClose();
  };
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
    ? <Button variant="primary" onClick={cancel}>{t("common.close")}</Button>
    : <><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{fileLoading ? null : session ? <Button variant="primary" busy={busy === "import-confirm"} disabled={selected.length === 0} onClick={confirm}>{t("accounts.confirmImport", { count: selected.length })}</Button> : <Button variant="primary" busy={busy === "import-preview"} disabled={!content.trim()} onClick={preview}>{t("accounts.preview")}</Button>}</>;
  const body = completed ? <div role="alert" className="relay-form import-failure-summary"><strong>{t("accounts.importIncomplete")}</strong><p>{t("accounts.importIncompleteHint", { count: completed.length })}</p><ul className="import-failure-list">{completed.map((failure) => <li key={failure.itemId}><div><strong>{failure.label || t("accounts.importUnknownAccount")}</strong><code title={t("accounts.importTechnicalCode")}>{failure.code}</code></div>{failure.identity ? <span>{failure.identity}</span> : null}<p>{importFailureReason(failure.code, t)}</p></li>)}</ul></div> : session ? <div className="import-preview"><div className="import-preview-heading"><div><strong>{t("accounts.importReady")}</strong><span>{t("accounts.importReadyHint", { selected: selected.length, total: session.preview.rows.length })}</span></div><StatusBadge status={selected.length ? "ready" : "warning"} label={t("accounts.selectedCount", { count: selected.length })} /></div><div className="relay-table-wrap"><table className="relay-table"><thead><tr><th><span className="sr-only">{t("accounts.selectImport")}</span></th><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("accounts.identity")}</th><th>{t("accounts.plan")}</th></tr></thead><tbody>{session.preview.rows.map((row) => {
    const badge = row.status === "invalid" ? "error" : row.status === "quota_failed" ? "warning" : row.status === "existing" ? "info" : "ready";
    return <tr key={row.itemId}><td><input type="checkbox" checked={selected.includes(row.itemId)} disabled={!row.selectable} aria-label={t("accounts.selectImportRow", { name: row.label })} onChange={() => toggle(row.itemId)} /></td><td><StatusBadge status={badge} label={t(`accounts.importStatus.${row.status}`, { defaultValue: row.status })} /></td><td>{row.label}{row.error ? <small className="error-text">{t("accounts.importIssue", { code: row.error.code })}</small> : row.warnings.length ? <small>{row.warnings.map((warning) => warning.code).join(", ")}</small> : null}</td><td><code>{row.identity}</code></td><td><AccountPlanBadge planType={row.plan ?? null} unknown="-" /></td></tr>;
  })}</tbody></table></div>{canImportToPool || localProxyOptions ? <div className="post-import-options"><span>{t("accounts.afterImport")}</span>{canImportToPool ? <label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addImportedToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label> : null}{localProxyOptions ? <label><input type="checkbox" checked={assignProxy} disabled={!proxyPool || proxyPool.free === 0 || selectedAccountCount === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignFreeAfterAdd")}</strong><small>{proxyPool ? t(proxyPool.free ? "proxies.importAssignmentHint" : "proxies.noFreeStored", { free: proxyPool.free, selected: selectedAccountCount, count: proxyPool.free }) : t("common.loading")}</small></span></label> : null}</div> : null}</div> : fileLoading ? <div className="import-file-loading" role="status" aria-live="polite"><span><Loader2 className="spin" aria-hidden /></span><div><strong>{t("accounts.readingImportFiles")}</strong><p>{t("accounts.readingImportFilesHint")}</p></div></div> : <div className="relay-form import-start"><button type="button" className="import-file-source" disabled={busy === "import-files"} onClick={() => void chooseFiles()}><span>{busy === "import-files" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />}</span><strong>{t("accounts.chooseImportFiles")}</strong><small>{t("accounts.importFileHint")}</small></button><div className="import-source-divider"><span>{t("accounts.orPaste")}</span></div><label className="relay-field"><span>{t("accounts.importData")}</span><textarea value={content} onChange={(event) => setContent(event.target.value)} placeholder={mode === "local" ? t("accounts.importPlaceholder") : t("accounts.remoteImportPlaceholder")} spellCheck={false} /></label><p className="form-note">{t("accounts.importFormatsHint")}</p></div>;
  return <Dialog wide title={t("accounts.import")} onClose={cancel} footer={footer}>{commandFailed ? <p role="alert" className="form-note error-text">{t("accounts.importCommandFailed")}</p> : null}{body}</Dialog>;
}

function AutomationDialog({ task, onClose }: { task: WakeTask | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [name, setName] = useState(task?.name ?? t("automations.defaultName"));
  const [automatic, setAutomatic] = useState(task?.executionPolicy !== "require_confirmation");
  const [windowKinds, setWindowKinds] = useState<WakeTask["windowKinds"]>(task?.windowKinds ?? ["primary", "secondary"]);
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [tags, setTags] = useState(task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : "");
  const [modelKind, setModelKind] = useState<WakeTask["modelPolicy"]["kind"]>(task?.modelPolicy.kind ?? "lightest_supported");
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const availableModels = useMemo(() => {
    const accounts = (runtime?.accounts ?? []).filter((account) => selectorKind !== "account_ids" || accountIds.includes(account.id));
    const modelSets = accounts.map((account) => account.models.filter((model) => (account.allowedModels.length === 0 || account.allowedModels.some((allowed) => allowed.toLowerCase() === model.toLowerCase())) && !account.excludedModels.some((excluded) => excluded.toLowerCase() === model.toLowerCase())));
    const models = selectorKind === "account_ids" && modelSets.length > 1
      ? modelSets[0].filter((model) => modelSets.slice(1).every((set) => set.some((candidate) => candidate.toLowerCase() === model.toLowerCase())))
      : modelSets.flat();
    return [...new Set([...(modelId ? [modelId] : []), ...models])].sort();
  }, [accountIds, modelId, runtime?.accounts, selectorKind]);
  const toggleWindow = (kind: WakeTask["windowKinds"][number]) => setWindowKinds((current) => current.includes(kind) ? current.filter((item) => item !== kind) : [...current, kind]);
  const toggleAccount = (id: string) => setAccountIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  const parsedTags = tags.split(",").map((tag) => tag.trim()).filter(Boolean);
  const valid = Boolean(name.trim() && windowKinds.length && (selectorKind !== "account_ids" || accountIds.length) && (selectorKind !== "tags" || parsedTags.length) && (modelKind !== "explicit" || modelId));
  const save = async () => {
    const now = Date.now();
    const accountSelector = selectorKind === "account_ids" ? { kind: selectorKind, values: accountIds } : selectorKind === "tags" ? { kind: selectorKind, values: parsedTags } : { kind: selectorKind };
    const modelPolicy = modelKind === "explicit" ? { kind: modelKind, value: modelId } : { kind: modelKind };
    const base = { ...defaultWakeInput(name), enabled: task?.enabled ?? true, accountSelector, windowKinds, modelPolicy, executionPolicy: automatic ? "automatic" as const : "require_confirmation" as const, jitterSeconds: task?.jitterSeconds ?? 0, maxAttemptsPerCycle: task?.maxAttemptsPerCycle ?? 1 };
    const remoteInput = task ? { ...task, ...base, updatedAtMs: now } : { ...base, id: "", trigger: { kind: "quota_full" }, fallbackSchedule: null, createdAtMs: now, updatedAtMs: now };
    const id = task ? `automation-update-${task.id}` : "automation-create";
    const ok = await perform(id, () => mode === "local" ? (task ? relayCommands.updateAutomation(task.id, base) : relayCommands.createAutomation(base)) : relayCommands.remoteAction({ type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) }, remoteInput), task ? "feedback.saved" : "feedback.automationAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} /></label><div className="form-row"><span>{t("automations.windows")}</span><label><input type="checkbox" checked={windowKinds.includes("primary")} onChange={() => toggleWindow("primary")} />{t("quota.primary")}</label><label><input type="checkbox" checked={windowKinds.includes("secondary")} onChange={() => toggleWindow("secondary")} />{t("quota.secondary")}</label></div><div className="relay-field"><span>{t("automations.accountSelection")}</span><OptionMenu className="field-option-menu" label={t("automations.accountSelection")} value={selectorKind} onChange={(value) => setSelectorKind(value as WakeTask["accountSelector"]["kind"])} options={[{ value: "all_eligible", label: t("automations.allEligible") }, { value: "account_ids", label: t("automations.selectedAccounts") }, { value: "tags", label: t("automations.matchingTags") }]} /></div>{selectorKind === "account_ids" ? <fieldset><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} />{account.label}</label>)}</div></fieldset> : null}{selectorKind === "tags" ? <label className="relay-field"><span>{t("automations.tags")}</span><input value={tags} onChange={(event) => setTags(event.target.value)} placeholder={t("automations.tagsPlaceholder")} /></label> : null}<div className="relay-field"><span>{t("automations.modelPolicy")}</span><OptionMenu className="field-option-menu" label={t("automations.modelPolicy")} value={modelKind} onChange={(value) => setModelKind(value as WakeTask["modelPolicy"]["kind"])} options={[{ value: "lightest_supported", label: t("automations.lightest") }, { value: "explicit", label: t("automations.explicitModel") }]} /></div>{modelKind === "explicit" ? <div className="relay-field"><span>{t("common.model")}</span><OptionMenu className="field-option-menu" label={t("common.model")} value={modelId} onChange={setModelId} options={[{ value: "", label: t("automations.selectModel") }, ...availableModels.map((model) => ({ value: model, label: model }))]} /></div> : null}<label className="toggle-row"><input type="checkbox" checked={automatic} onChange={(event) => setAutomatic(event.target.checked)} /><span>{t("automations.automatic")}</span></label><p className="form-note">{t("automations.fixedPrompt")}</p></div></Dialog>;
}

function RemoteDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [allowInsecure, setAllowInsecure] = useState(false);
  const [confirmIdentityChange, setConfirmIdentityChange] = useState(false);
  const insecure = baseUrl.trim().toLowerCase().startsWith("http://");
  const connect = async () => { const ok = await perform("remote-connect", () => relayCommands.connectRemote({ baseUrl, managementToken: token, allowInsecureHttp: insecure && allowInsecure, confirmIdentityChange }), "feedback.connected"); if (ok) onClose(); };
  return <Dialog title={t("remote.connectExisting")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "remote-connect"} disabled={!baseUrl || !token || (insecure && !allowInsecure)} onClick={connect}>{t("remote.testAndConnect")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={token} onChange={setToken} />{insecure ? <label className="check-line"><input type="checkbox" checked={allowInsecure} onChange={(event) => setAllowInsecure(event.target.checked)} /><span>{t("remote.allowInsecure")}</span></label> : null}<label className="check-line"><input type="checkbox" checked={confirmIdentityChange} onChange={(event) => setConfirmIdentityChange(event.target.checked)} /><span>{t("remote.confirmIdentityChange")}</span></label><p className="form-note">{t("remote.identityHint")}</p></div></Dialog>;
}

function DeployDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [url, setUrl] = useState("");
  const [plan, setPlan] = useState<{ directory: string; managementToken: string; vaultKey: string; composeCommand: string } | null>(null);
  const generate = async () => { const result: { current: typeof plan } = { current: null }; const ok = await perform("remote-deploy", async () => { result.current = await relayCommands.prepareRemoteDeployment(url); }, "feedback.deploymentPrepared"); if (ok) setPlan(result.current); };
  return <Dialog title={t("remote.deployNew")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>{!plan ? <Button variant="primary" busy={busy === "remote-deploy"} disabled={!url} onClick={generate}>{t("remote.generate")}</Button> : null}</>}>{plan ? <div className="deployment-result"><StatusBadge status="ready" label={t("common.ready")} /><label><span>{t("remote.bundlePath")}</span><code>{plan.directory}</code></label><div className="relay-field"><span>{t("remote.token")}</span><div className="endpoint-line"><input aria-label={t("remote.token")} type="password" value={plan.managementToken} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.managementToken)}>{t("common.copy")}</Button></div></div><div className="relay-field"><span>{t("remote.vaultKey")}</span><div className="endpoint-line"><input aria-label={t("remote.vaultKey")} type="password" value={plan.vaultKey} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.vaultKey)}>{t("common.copy")}</Button></div></div><label><span>{t("remote.command")}</span><code>{plan.composeCommand}</code></label><p>{t("remote.secretOnce")}</p><p>{t("remote.deployHint")}</p></div> : <label className="relay-field"><span>{t("remote.publicUrl")}</span><input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://relay.example.com" /></label>}</Dialog>;
}

function ReadyApiDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy, setMode } = useRelayState();
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const save = async () => {
    const ok = await perform("ready-save", async () => {
      if (provider.kind === "zenith") {
        await saveKey(provider.apiKey);
        return;
      }
      const created = await relayCommands.createSource(apiProviderSourceInput(provider)) as { id: string };
      await relayCommands.setPoolMembership([], [created.id], true);
    }, "feedback.connected");
    if (!ok) return;
    if (provider.kind !== "zenith") setMode("local");
    onClose();
  };
  return <Dialog wide title={t("apiProviders.connect")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "ready-save"} disabled={!apiProviderReady(provider)} onClick={save}>{t("apiProviders.connectAction")}</Button></>}><ApiProviderForm value={provider} onChange={setProvider} /></Dialog>;
}

function TopUpDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [amount, setAmount] = useState("10");
  const submit = async () => {
    const prepared = await prepareTopUpAmount(amount);
    if (!prepared.valid) return;
    const ok = await perform("ready-topup", () => createSavedTopUpIntentAndOpen(prepared.amountCents), "feedback.topUpOpened");
    if (ok) onClose();
  };
  return <Dialog title={t("readyApi.topUp")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "ready-topup"} disabled={!amount.trim()} onClick={submit}>{t("readyApi.openTopUp")}</Button></>}><label className="relay-field"><span>{t("readyApi.amount")}</span><input inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} /></label></Dialog>;
}

function safeHost(value: string) {
  try { return new URL(value).host; } catch { return value; }
}

function accountHealthTone(health: string) {
  if (health === "healthy") return "ready";
  if (health === "blocked" || health === "unhealthy") return "error";
  return "warning";
}

function compareAccounts(left: AccountSummary, right: AccountSummary) {
  return automaticAccountTier(left) - automaticAccountTier(right)
    || compareQuotaDescending(quotaFloor(left), quotaFloor(right))
    || left.label.localeCompare(right.label);
}

function automaticAccountTier(account: AccountSummary) {
  if (accountErrorCode(account)) return 4;
  if (account.routingExclusion === "free_plan_policy") return 3;
  if (!accountPoolReady(account)) return 2;
  return quotaFloor(account) == null ? 1 : 0;
}

function accountParticipates(account: AccountSummary) {
  return account.inPool;
}

function accountRouted(account: AccountSummary) {
  return account.inPool && account.routingExclusion == null;
}

function accountPoolReady(account: AccountSummary) {
  return accountRouted(account)
    && account.enabled
    && !account.draining
    && account.secretAvailable
    && account.proxyAvailable !== false
    && ["unknown", "healthy", "degraded"].includes(account.health)
    && ![account.quota.primary, account.quota.secondary].some((window) => window?.availableBasisPoints === 0);
}

function quotaFloor(account: AccountSummary) {
  const values = [account.quota.primary, account.quota.secondary]
    .map((window) => window?.availableBasisPoints)
    .filter((value): value is number => value != null);
  return values.length ? Math.min(...values) : null;
}

function compareQuotaDescending(left: number | null, right: number | null) {
  if (left == null && right == null) return 0;
  if (left == null) return 1;
  if (right == null) return -1;
  return right - left;
}

function accountErrorCode(account: AccountSummary) {
  const auth = typeof account.authState === "string" ? { state: account.authState } : account.authState;
  if (auth.state === "requires_reauth") return auth.reason ? "auth_" + auth.reason : "auth_requires_reauth";
  const stored = account.lastErrorCode?.trim() || account.quota.error?.code.trim();
  if (stored) return stored;
  if (auth.state === "error") return "auth_error";
  if (!account.secretAvailable) return "credentials_missing";
  if (account.health === "blocked" || account.health === "unhealthy") return "health_" + account.health;
  return null;
}

function accountIsTerminallyUnusable(account: AccountSummary) {
  const auth = typeof account.authState === "string" ? { state: account.authState } : account.authState;
  if (auth.state === "requires_reauth" || auth.state === "error" || !account.secretAvailable) return true;
  const code = (account.lastErrorCode?.trim() || account.quota.error?.code.trim() || "").toLowerCase();
  return new Set([
    "credentials_missing",
    "invalid_chatgpt_account_id",
    "models_forbidden",
    "models_invalid_access_token",
    "models_invalid_account_id",
    "models_unauthorized",
    "quota_forbidden",
    "quota_unauthorized",
  ]).has(code) || code.startsWith("auth_");
}

function accountErrorLabel(code: string, t: TFunction) {
  const normalized = code.toLocaleLowerCase();
  if (/reused_refresh_token|refresh_token_reused/.test(normalized)) return t("accounts.errors.reusedRefreshToken");
  if (/expired_refresh_token|refresh_token_expired/.test(normalized)) return t("accounts.errors.expiredRefreshToken");
  if (/invalidated_refresh_token|refresh_token_invalidated|token_invalidated/.test(normalized)) return t("accounts.errors.invalidatedRefreshToken");
  if (/invalid_grant/.test(normalized)) return t("accounts.errors.invalidGrant");
  if (/invalid_grant|requires_reauth|refresh_token|auth_error|unauthorized/.test(normalized)) return t("accounts.errors.requiresReauth");
  if (/credential|secret/.test(normalized)) return t("accounts.errors.credentialsMissing");
  if (/forbidden|blocked/.test(normalized)) return t("accounts.errors.blocked");
  if (/rate.?limit|too_many/.test(normalized)) return t("accounts.errors.rateLimited");
  if (/transport|timeout|network|connect/.test(normalized)) return t("accounts.errors.connection");
  if (/quota/.test(normalized)) return t("accounts.errors.quota");
  if (/response|parse|decode|malformed/.test(normalized)) return t("accounts.errors.invalidResponse");
  return t("accounts.errors.unknown");
}

function subscriptionEndDisplay(activeUntilMs: number | null, locale: string, t: TFunction, nowMs: number) {
  if (activeUntilMs == null) return { date: t("accounts.subscriptionEndUnknown"), relative: null };
  const value = new Intl.DateTimeFormat(locale, { day: "2-digit", month: "2-digit", year: "numeric" }).format(activeUntilMs);
  const deltaMs = activeUntilMs - nowMs;
  const absoluteMs = Math.abs(deltaMs);
  const expired = deltaMs <= 0;
  const date = t(expired ? "accounts.subscriptionEnded" : "accounts.subscriptionUntil", { value });
  if (!expired && absoluteMs < 24 * 60 * 60_000) {
    const totalSeconds = Math.max(0, Math.ceil(absoluteMs / 1_000));
    const hours = Math.floor(totalSeconds / 3_600);
    const minutes = Math.floor(totalSeconds % 3_600 / 60);
    const seconds = totalSeconds % 60;
    const clock = [hours, minutes, seconds].map((part) => String(part).padStart(2, "0")).join(":");
    return { date, relative: t("accounts.subscriptionCountdown", { value: clock }) };
  }
  const unit = absoluteMs < 48 * 60 * 60_000 ? "hour" : "day";
  const unitMs = unit === "hour" ? 60 * 60_000 : 24 * 60 * 60_000;
  const count = Math.max(1, Math.ceil(absoluteMs / unitMs)) * (expired ? -1 : 1);
  return { date, relative: new Intl.RelativeTimeFormat(locale, { numeric: "always" }).format(count, unit) };
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

function matchesQuery(query: string, ...values: Array<string | string[] | null | undefined>) {
  const normalized = query.trim().toLocaleLowerCase();
  return !normalized || values.flatMap((value) => Array.isArray(value) ? value : value ?? []).some((value) => value.toLocaleLowerCase().includes(normalized));
}

function parseList(value: string) {
  return [...new Set(value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean))];
}

function NoResults() {
  const { t } = useTranslation();
  return <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
}
