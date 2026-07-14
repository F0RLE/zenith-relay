import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { ArrowDown, ArrowUp, ArrowUpDown, CalendarDays, CirclePause, Copy, CreditCard, Download, Eye, EyeOff, Layers3, LayoutGrid, List, Loader2, LogIn, Network, Pencil, Play, Plus, Power, RefreshCw, Rows3, ShieldAlert, Trash2, Upload, UserRoundX, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createSavedTopUpIntentAndOpen, prepareTopUpAmount, resetKey, saveKey } from "../../../../tauri";
import { defaultWakeInput, relayCommands } from "../../api/commands";
import type { AccountExportFormat, AccountSummary, ConfirmAccountImportResponse, ImportSession, OAuthFlow, ProxyAssignmentResult, RuntimeSnapshot, SourceSummary, WakeTask } from "../../api/types";
import {
  Button,
  ActionMenu,
  ActionMenuItem,
  Dialog,
  EmptyState,
  IconButton,
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
  formatAccountPlan,
} from "../../components/Ui";
import type { ApiSourceRole } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type View = "sources" | "accounts" | "automations" | "remote" | "api";
type DialogKind = "source" | "oauth" | "automation" | "remote" | "deploy" | "ready" | "topup" | "accountProxy" | "bulkProxies" | "accountExport" | null;
type AccountSort = "pool" | "participation" | "primary" | "secondary" | "primary_reset" | "secondary_reset" | "plan" | "name";
type AccountLayout = "compact" | "list" | "grid";
type SortDirection = "asc" | "desc";
type ParticipationFilter = "all" | "included" | "excluded";
type ImportFailure = { itemId: string; code: string; label?: string; identity?: string };

const accountExportFormats: Array<{ value: AccountExportFormat; label: string }> = [
  { value: "sub2api", label: "sub2api" },
  { value: "cpa", label: "CPA" },
  { value: "cockpit", label: "Cockpit" },
  { value: "9router", label: "9router" },
  { value: "codex", label: "Codex" },
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
        ...(supports("sources") ? [{ id: "sources", label: t("connections.sources") }] : []),
        ...(supports("wake_tasks") ? [{ id: "automations", label: t("connections.automations") }] : []),
        ...(mode === "remote" ? [{ id: "remote", label: t("connections.remoteServer") }] : []),
      ];

  const primaryLabel = view === "accounts"
    ? mode === "local" ? t("accounts.signIn") : t("connections.import")
    : view === "sources"
      ? t("sources.add")
      : view === "automations"
        ? t("automations.add")
        : view === "remote"
          ? runtime ? t("remote.refresh") : t("remote.connect")
          : readyState?.providerActive ? t("readyApi.topUp") : t("readyApi.connect");

  const primaryAction = () => {
    if (view === "accounts" && !canImportAccounts) return;
    if (view === "accounts" && mode === "remote") {
      onImport();
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
      view === "accounts" ? "oauth"
        : view === "sources" ? "source"
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
            <Button variant="primary" icon={view === "accounts" ? mode === "local" ? <LogIn aria-hidden /> : <Upload aria-hidden /> : view === "remote" && runtime ? <RefreshCw aria-hidden /> : <Plus aria-hidden />} disabled={view === "accounts" && !canImportAccounts} title={view === "accounts" && !canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={primaryAction}>
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
      {view === "accounts" ? <AccountsTable query={query} onQuery={setQuery} canImport={canImportAccounts} canManageProxies={canManageProxies} canExport={canExportAccounts} canRevealIdentity={canRevealAccountIdentity} onImport={onImport} onSignIn={() => setDialog("oauth")} onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }} onBulkProxies={(accountIds) => { setBulkProxyAccountIds(accountIds); setDialog("bulkProxies"); }} onExport={(accountIds) => { setExportAccountIds(accountIds); setDialog("accountExport"); }} /> : null}
      {view === "automations" ? <AutomationsTable query={query} onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}
      {view === "remote" ? <RemoteView onConnect={() => setDialog("remote")} onDeploy={() => setDialog("deploy")} /> : null}
      {view === "api" ? <ReadyApiView connected={Boolean(readyState?.providerActive)} onConnect={() => setDialog("ready")} onTopUp={() => setDialog("topup")} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {dialog === "oauth" ? <OAuthDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "remote" ? <RemoteDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "deploy" ? <DeployDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "ready" ? <ReadyApiDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "topup" ? <TopUpDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "accountProxy" && proxyAccount ? <AccountProxyDialog account={proxyAccount} onClose={() => { setDialog(null); setProxyAccount(null); }} /> : null}
      {dialog === "bulkProxies" ? <BulkProxyDialog accountIds={bulkProxyAccountIds} onClose={() => setDialog(null)} /> : null}
      {dialog === "accountExport" ? <AccountExportDialog accountIds={exportAccountIds} onClose={() => { setDialog(null); setExportAccountIds([]); }} /> : null}
      {busy ? <span className="sr-only" aria-live="polite">{t("common.working")}</span> : null}
    </section>
  );
}

function SourcesTable({ query, onEdit }: { query: string; onEdit: (source: SourceSummary) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canTest = mode !== "remote" || runtime?.capabilities.features.includes("diagnostics");
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(query, source.name, source.baseUrl, source.wireApi, source.models));
  if (!sources.length) return <NoResults />;
  return (
    <div className="relay-table-wrap">
      <table className="relay-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.protocol")}</th><th>{t("common.models")}</th><th>{t("pool.priority")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => (
          <tr key={source.id}>
            <td><StatusBadge status={source.enabled && source.secretAvailable ? "ready" : "disabled"} label={source.enabled ? t("common.enabled") : t("common.disabled")} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td>{source.wireApi}</td>
            <td>{source.models.length}</td>
            <td>{source.priority}</td>
            <td className="row-actions">
              <IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={!canTest || busy === `test-${source.id}`} title={!canTest ? t("remote.capabilityUnavailable") : t("common.test")} onClick={() => perform(`test-${source.id}`, () => mode === "local" ? relayCommands.testSource(source.id) : relayCommands.remoteAction({ type: "test_source", id: source.id }), "feedback.checked")} />
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <ActionMenu>
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => mode === "local" ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")}>{source.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("sources.deleteConfirm"))) void perform(`delete-${source.id}`, () => mode === "local" ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"); }}>{t("common.delete")}</ActionMenuItem>
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
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const [selected, setSelected] = useState<string[]>([]);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [sortBy, setSortBy] = useState<AccountSort>("pool");
  const [sortDirection, setSortDirection] = useState<SortDirection>("desc");
  const [layout, setLayout] = useState<AccountLayout>(() => {
    const saved = localStorage.getItem("relay.connections.accountLayout");
    return saved === "compact" || saved === "grid" ? saved : "list";
  });
  const [revealedIdentities, setRevealedIdentities] = useState<Record<string, string>>({});
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const allAccounts = runtime?.accounts ?? [];
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
  const planOrder = new Map(plans.map((plan, index) => [plan.id, index]));
  useEffect(() => setSelected((current) => current.filter((id) => allAccounts.some((account) => account.id === id))), [runtime?.accounts]);
  useEffect(() => localStorage.setItem("relay.connections.accountLayout", layout), [layout]);
  useEffect(() => { setRevealedIdentities({}); setPlanFilter("all"); setParticipationFilter("all"); }, [mode]);
  if (!runtime?.accounts.length) {
    return <EmptyState title={t("accounts.emptyTitle")} description={t("accounts.emptyDescription")} action={<div className="inline-actions">{mode === "local" ? <Button variant="primary" onClick={onSignIn}>{t("accounts.signIn")}</Button> : null}<Button variant={mode === "local" ? "secondary" : "primary"} disabled={!canImport} title={!canImport ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>{t("accounts.import")}</Button></div>} />;
  }
  const accounts = [...runtime.accounts]
    .filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType, account.models))
    .filter((account) => activePlan === "all" || (activePlan === "errors" ? Boolean(accountErrorCode(account)) : accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan))
    .filter((account) => participationFilter === "all" || (participationFilter === "included") === accountParticipates(account))
    .sort((left, right) => compareAccounts(left, right, sortBy, sortDirection, planOrder, plans.length));
  const filtersActive = Boolean(query.trim()) || activePlan !== "all" || participationFilter !== "all";
  const filtersHideAccounts = filtersActive && accounts.length !== allAccounts.length;
  const exportIds = selected.length ? selected : allAccounts.map((account) => account.id);
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.includes(account.id));
  const toggleSelected = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const toggleAllVisible = (checked: boolean) => setSelected(checked ? [...new Set([...selected, ...accounts.map((account) => account.id)])] : selected.filter((id) => !accounts.some((account) => account.id === id)));
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
    const selectedAccounts = allAccounts.filter((account) => selected.includes(account.id));
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
    if (ok) setSelected((current) => current.filter((id) => !accountIds.includes(id)));
    return ok;
  };
  const deleteSelected = () => {
    const accountIds = allAccounts.filter((account) => selected.includes(account.id)).map((account) => account.id);
    if (accountIds.length && window.confirm(t("accounts.deleteSelectedConfirm", { count: accountIds.length }))) {
      void deleteAccounts(accountIds, "delete-selected-accounts");
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
    if (accountIds.length && window.confirm(t("accounts.deleteNonWorkingConfirm", { count: accountIds.length }))) {
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
        {selected.length ? <span>{t("accounts.selectedCount", { count: selected.length })}</span> : <label className="search-field account-search"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => onQuery(event.target.value)} placeholder={t("common.search")} /></label>}
      </div>
      <div className="account-command-actions">
        {selected.length ? <>
          <IconButton label={t("accounts.includeSelectedInPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <Play aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(true)} />
          <IconButton label={t("accounts.excludeSelectedFromPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <CirclePause aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(false)} />
          <IconButton label={t("accounts.exportSelected", { count: selected.length })} icon={<Download aria-hidden />} disabled={!canExport || Boolean(busy)} title={!canExport ? t("remote.capabilityUnavailable") : t("accounts.exportSelected", { count: selected.length })} onClick={() => onExport(exportIds)} />
          <IconButton className="danger" label={t("accounts.deleteSelected")} icon={busy === "delete-selected-accounts" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={deleteSelected} />
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} />
        </> : <>
          <div className="account-sort-controls">
            <label className="account-sort-select"><span className="sr-only">{t("accounts.sort.label")}</span><ArrowUpDown aria-hidden /><select aria-label={t("accounts.sort.label")} value={sortBy} onChange={(event) => setSortBy(event.target.value as AccountSort)}><option value="pool">{t("accounts.sort.pool")}</option><option value="participation">{t("accounts.sort.participation")}</option><option value="primary">{t("accounts.sort.primary")}</option><option value="secondary">{t("accounts.sort.secondary")}</option><option value="primary_reset">{t("accounts.sort.primaryReset")}</option><option value="secondary_reset">{t("accounts.sort.secondaryReset")}</option><option value="plan">{t("accounts.sort.plan")}</option><option value="name">{t("accounts.sort.name")}</option></select></label>
            {sortBy !== "pool" ? <IconButton label={sortDirection === "desc" ? t("accounts.sort.descending") : t("accounts.sort.ascending")} icon={sortDirection === "desc" ? <ArrowDown aria-hidden /> : <ArrowUp aria-hidden />} onClick={() => setSortDirection((value) => value === "desc" ? "asc" : "desc")} /> : null}
          </div>
          <div className="view-layout-switcher" role="group" aria-label={t("accounts.layout.label")}><IconButton label={t("accounts.layout.compact")} aria-pressed={layout === "compact"} onClick={() => setLayout("compact")} icon={<Rows3 aria-hidden />} /><IconButton label={t("accounts.layout.list")} aria-pressed={layout === "list"} onClick={() => setLayout("list")} icon={<List aria-hidden />} /><IconButton label={t("accounts.layout.grid")} aria-pressed={layout === "grid"} onClick={() => setLayout("grid")} icon={<LayoutGrid aria-hidden />} /></div>
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
        return <button key={value} type="button" aria-pressed={participationFilter === value} aria-label={t("accounts.participationFilterOption", { state: t(`accounts.participation.${value}`), count })} onClick={() => setParticipationFilter(value)}><span>{t(`accounts.participation.${value}`)}</span><small>{count}</small></button>;
      })}
    </div>
    {plans.length > 1 ? <div className="account-plan-filters" role="group" aria-label={t("accounts.filterByPlan")}>
      <span>{t("accounts.plan")}</span>
      <button type="button" aria-pressed={activePlan === "all"} aria-label={t("accounts.planFilterOption", { plan: t("accounts.allPlans"), count: allAccounts.length })} onClick={() => setPlanFilter("all")}><span>{t("accounts.allPlans")}</span><small>{allAccounts.length}</small></button>
      {errorCount ? <button type="button" className="error" aria-pressed={activePlan === "errors"} aria-label={t("accounts.planFilterOption", { plan: t("accounts.errorsOnly"), count: errorCount })} onClick={() => setPlanFilter("errors")}><ShieldAlert aria-hidden /><span>{t("accounts.errorsOnly")}</span><small>{errorCount}</small></button> : null}
      {plans.map((plan) => <button key={plan.id} type="button" aria-pressed={activePlan === plan.id} aria-label={t("accounts.planFilterOption", { plan: plan.label, count: plan.count })} onClick={() => setPlanFilter(plan.id)}><span>{plan.label}</span><small>{plan.count}</small></button>)}
    </div> : null}
    </div>
    {filtersHideAccounts ? <div className="account-filter-summary" role="status" aria-live="polite"><span>{t("accounts.filterSummary", { visible: accounts.length, total: allAccounts.length })}</span><button type="button" onClick={() => { onQuery(""); setPlanFilter("all"); setParticipationFilter("all"); }}><X aria-hidden /><span>{t("accounts.clearFilters")}</span></button></div> : null}
    {accounts.length ? <div className="account-list" role="list" aria-label={t("connections.accounts")} data-layout={layout}>
      {accounts.map((account) => {
        const errorCode = accountErrorCode(account);
        const participates = accountParticipates(account);
        const excludedByFreePolicy = account.routingExclusion === "free_plan_policy";
        const subscriptionEnded = account.subscription.activeUntilMs != null && account.subscription.activeUntilMs <= Date.now();
        const subscriptionEnd = subscriptionEndDisplay(account.subscription.activeUntilMs, i18n.resolvedLanguage ?? i18n.language, t, nowMs);
        return (
        <article key={account.id} className={`account-card${selected.includes(account.id) ? " selected" : ""}`} role="listitem">
          <div className="account-card-main">
            <input type="checkbox" aria-label={t("accounts.select", { name: account.label })} checked={selected.includes(account.id)} onChange={() => toggleSelected(account.id)} />
            <div className="account-identity">
              <strong className={revealedIdentities[account.id] ? "revealed" : undefined} title={revealedIdentities[account.id] ?? account.label}>{revealedIdentities[account.id] ?? account.label}</strong>
              <div>
                <span className={`account-health ${accountHealthTone(account.health)}`}>{t(`health.${account.health}`, { defaultValue: account.health })}</span>
                {account.inPool && account.priority !== 0 ? <span className="account-priority" title={t("pool.priorityHelp")}>{t("pool.priorityValue", { value: account.priority })}</span> : null}
              </div>
            </div>
            <div className="account-facts">
              <div className="account-fact account-fact-plan"><CreditCard className="account-fact-icon" aria-hidden /><span>{t("accounts.plan")}</span><strong>{formatAccountPlan(account.subscription.planType, t("common.unknown"))}</strong></div>
              <div className="account-fact account-fact-proxy"><Network className="account-fact-icon" aria-hidden /><span>{t("proxies.proxy")}</span><button type="button" className="proxy-status-button" disabled={!canManageProxies} title={!canManageProxies ? t("remote.capabilityUnavailable") : t("proxies.changeAccount")} onClick={() => onProxy(account)}><StatusBadge status={account.proxyAvailable === false ? "error" : account.proxyMode === "account" ? "info" : "ready"} label={account.proxyAvailable === false && account.proxyMode === "direct" ? t("proxies.modes.blocked") : t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></button></div>
              <div className="account-fact account-fact-pool"><Layers3 className="account-fact-icon" aria-hidden /><span>{t("accounts.poolParticipation")}</span><label className="account-pool-switch" title={excludedByFreePolicy ? t("accounts.participation.freePolicyHint") : participates ? t("accounts.excludeFromPool") : t("accounts.includeInPool")}><input type="checkbox" role="switch" checked={participates} disabled={busy === `pool-${account.id}`} aria-label={t("accounts.poolParticipationFor", { name: account.label })} onChange={(event) => void perform(`pool-${account.id}`, () => updateParticipation(account, event.target.checked), "feedback.saved")} /><strong>{excludedByFreePolicy && participates ? t("accounts.participation.freePolicy") : participates ? t("accounts.participation.included") : t("accounts.participation.excluded")}</strong></label></div>
            </div>
            <div className="account-row-action-list">
              <ActionMenu className="account-row-menu">
                <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport || !account.secretAvailable} onClick={() => onExport([account.id])}>{t("accounts.exportOne", { name: account.label })}</ActionMenuItem>
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void perform(`enable-${account.id}`, () => mode === "local" ? relayCommands.setAccountEnabled(account.id, !account.enabled) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled: !account.enabled }), "feedback.saved"); }}>{account.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("accounts.deleteConfirm"))) void perform(`delete-${account.id}`, () => mode === "local" ? relayCommands.deleteAccount(account.id) : relayCommands.remoteAction({ type: "delete_account", id: account.id }), "feedback.deleted"); }}>{t("common.delete")}</ActionMenuItem>
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

function AccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, busy, perform } = useRelayState();
  const [proxyUrl, setProxyUrl] = useState("");
  const apply = async (value: string | null) => {
    const ok = await perform(`proxy-${account.id}`, () => mode === "local"
      ? relayCommands.setAccountProxy(account.id, value)
      : relayCommands.remoteAction({ type: "set_account_proxy", id: account.id }, { proxyUrl: value }), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("proxies.accountTitle", { name: account.label })} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>{account.proxyMode === "account" ? <Button variant="secondary" busy={busy === `proxy-${account.id}`} onClick={() => apply(null)}>{t("proxies.useInherited")}</Button> : null}<Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!proxyUrl.trim()} onClick={() => apply(proxyUrl.trim())}>{t("common.save")}</Button></>}><div className="relay-form"><div className="proxy-current"><span>{t("proxies.currentMode")}</span><StatusBadge status={account.proxyAvailable === false ? "error" : "ready"} label={t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></div><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder="user:password@us-proxy.example:8080" /><p className="form-note">{t("proxies.savedHidden")}</p></div></Dialog>;
}

function BulkProxyDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
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
      response = mode === "local"
        ? await relayCommands.assignAccountProxies(selectedAccountIds, proxyUrls)
        : await relayCommands.remoteAction({ type: "assign_account_proxies" }, { accountIds: selectedAccountIds, proxyUrls }) as ProxyAssignmentResult;
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
                <td className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("automations.deleteConfirm"))) void perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"); }}>{t("common.delete")}</ActionMenuItem></ActionMenu></td>
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
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.gateway.running ? "ready" : "warning"} label={runtime.gateway.running ? t("common.online") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="danger" onClick={() => { if (window.confirm(t("remote.disconnectConfirm"))) void perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected"); }}>{t("remote.disconnect")}</Button></div></section>;
}

function ReadyApiView({ connected, onConnect, onTopUp }: { connected: boolean; onConnect: () => void; onTopUp: () => void }) {
  const { t } = useTranslation();
  const { readyStats, perform } = useRelayState();
  return <section className="ready-api-connection"><div className="recommended-line"><div><strong>Zenith API</strong><small>https://api.zenithmarket.dev/v1</small></div><span>{t("common.recommended")}</span></div><StatusBadge status={connected ? "ready" : "warning"} label={connected ? t("common.connected") : t("common.notConfigured")} /><p>{t("readyApi.connectionHint")}</p>{connected ? <><dl className="detail-list"><div><dt>{t("readyApi.balance")}</dt><dd>{readyStats?.balance ?? "-"}</dd></div><div><dt>{t("usage.requests")}</dt><dd>{readyStats?.requestsDisplay ?? readyStats?.requests ?? "-"}</dd></div></dl><div className="inline-actions"><Button variant="secondary" onClick={onTopUp}>{t("readyApi.topUp")}</Button><Button variant="secondary" onClick={onConnect}>{t("readyApi.updateKey")}</Button><Button variant="danger" onClick={() => { if (window.confirm(t("readyApi.disconnectConfirm"))) void perform("ready-disconnect", resetKey, "feedback.disconnected"); }}>{t("remote.disconnect")}</Button></div></> : <Button variant="primary" onClick={onConnect}>{t("readyApi.connect")}</Button>}</section>;
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
  return <Dialog wide title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!source && !apiKey.trim()} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>}><form id="source-form" className="relay-form" onSubmit={submit}><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label><label className="relay-field"><span>{t("sources.protocol")}</span><select value={wireApi} onChange={(event) => setWireApi(event.target.value as SourceSummary["wireApi"])}><option value="responses">Responses API</option><option value="chat_completions">Chat Completions</option></select></label><SecretField label={source ? t("sources.replaceKey") : t("sources.apiKey")} value={apiKey} onChange={setApiKey} /><label className="relay-field"><span>{t("common.models")}</span><input value={models} onChange={(event) => setModels(event.target.value)} placeholder="gpt-5.4, gpt-5.4-mini" /></label><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><div className="settings-row"><label><span>{t("sources.poolRole")}</span><select value={role} onChange={(event) => setRole(event.target.value as ApiSourceRole)}><option value="primary">{t("sources.roles.primary")}</option><option value="stabilizer">{t("sources.roles.stabilizer")}</option><option value="reserve">{t("sources.roles.reserve")}</option></select><small>{t(`sources.roleHints.${role}`)}</small></label><label><span>{t("pool.trafficShare")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label></div></form></Dialog>;
}

function OAuthDialog({ onClose }: { onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const { perform, busy } = useRelayState();
  const [flow, setFlow] = useState<OAuthFlow | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");
  const [loginId, setLoginId] = useState("");
  const start = async () => {
    const result: { current: OAuthFlow | null } = { current: null };
    const ok = await perform("oauth-start", async () => { result.current = await relayCommands.startOAuth(); });
    if (ok) setFlow(result.current);
  };
  const resume = async () => {
    const result: { current: OAuthFlow | null } = { current: null };
    const ok = await perform("oauth-resume", async () => { result.current = await relayCommands.resumeOAuth(loginId.trim()); });
    if (ok) setFlow(result.current);
  };
  const finish = async () => {
    if (!flow) return;
    const ok = await perform("oauth-complete", async () => {
      if (callbackUrl.trim()) await relayCommands.submitOAuthCallback(flow.loginId, callbackUrl.trim());
      else await relayCommands.oauthStatus(flow.loginId);
      await relayCommands.completeOAuth(flow.loginId);
    }, "feedback.accountAdded");
    if (ok) onClose();
  };
  const cancel = async () => {
    if (flow) await perform("oauth-cancel", () => relayCommands.cancelOAuth(flow.loginId));
    onClose();
  };
  const expiresAt = flow ? new Intl.DateTimeFormat(i18n.language, { timeStyle: "short" }).format(new Date(flow.expiresAtMs)) : "";
  return <Dialog
    title={t("accounts.signIn")}
    onClose={cancel}
    footer={<><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{flow ? <Button variant="primary" busy={busy === "oauth-complete"} onClick={finish}>{t("accounts.finishSignIn")}</Button> : <Button variant="primary" busy={busy === "oauth-start"} onClick={start}>{t("accounts.openSignIn")}</Button>}</>}
  >
    {flow ? <div className="relay-form">
      <p>{t("accounts.browserOpened")}</p>
      <label className="relay-field"><span>{t("accounts.callbackUrl")}</span><input value={callbackUrl} onChange={(event) => setCallbackUrl(event.target.value)} placeholder={flow.redirectUri} /></label>
      <a href={flow.authorizationUrl} target="_blank" rel="noreferrer">{t("accounts.reopenSignIn")}</a>
      <small>{t("accounts.oauthExpires", { value: expiresAt })}</small>
    </div> : <div className="relay-form oauth-intro">
      <p>{t("accounts.oauthDescription")}</p>
      <details className="oauth-resume"><summary>{t("accounts.resumeExisting")}</summary><label className="relay-field"><span>{t("accounts.resumeLoginId")}</span><div className="inline-actions"><input value={loginId} onChange={(event) => setLoginId(event.target.value)} /><Button variant="secondary" busy={busy === "oauth-resume"} disabled={!loginId.trim()} onClick={resume}>{t("common.resume")}</Button></div></label></details>
    </div>}
  </Dialog>;
}

export function ImportDialog({ initialPaths, onClose }: { initialPaths?: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [content, setContent] = useState("");
  const [session, setSession] = useState<ImportSession | null>(null);
  const [resumeId, setResumeId] = useState("");
  const [ownedSessionId, setOwnedSessionId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [commandFailed, setCommandFailed] = useState(false);
  const [completed, setCompleted] = useState<ImportFailure[] | null>(null);
  const activeSessionId = useRef<string | null>(null);
  const initialPreviewStarted = useRef(false);
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
        setResumeId(started.sessionId);
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
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("import-files", async () => {
      result.current = mode === "local"
        ? await relayCommands.previewImportFiles(paths)
        : await relayCommands.previewRemoteImportFiles(paths);
    });
    if (ok && result.current) acceptSession(result.current);
    else if (!ok) setCommandFailed(true);
  };
  const resume = async () => {
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("import-resume", async () => { result.current = await relayCommands.resumeImport(resumeId.trim()); });
    if (ok && result.current) acceptSession(result.current);
    else if (!ok) setCommandFailed(true);
  };
  const confirm = async () => {
    if (!session) return;
    if (mode === "local") {
      const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
      const ok = await perform("import-confirm", async () => { result.current = await relayCommands.confirmImport(session.sessionId, selected); });
      if (!ok) {
        setSession(null);
        setSelected([]);
        setCommandFailed(true);
        activeSessionId.current = null;
        return;
      }
      const failures = collectImportFailures(result.current, session);
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
        { sessionId: session.sessionId, selectedItemIds: selected, probeMetadata: true },
      ) as Awaited<ReturnType<typeof relayCommands.confirmImport>>;
    }, "feedback.accountAdded");
    if (!ok) {
      setCommandFailed(true);
      return;
    }
    const failures = collectImportFailures(result.current, session);
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
  const footer = completed
    ? <Button variant="primary" onClick={cancel}>{t("common.close")}</Button>
    : <><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{session ? <Button variant="primary" busy={busy === "import-confirm"} disabled={selected.length === 0} onClick={confirm}>{t("accounts.confirmImport", { count: selected.length })}</Button> : <Button variant="primary" busy={busy === "import-preview"} disabled={!content.trim()} onClick={preview}>{t("accounts.preview")}</Button>}</>;
  const body = completed ? <div role="alert" className="relay-form import-failure-summary"><strong>{t("accounts.importIncomplete")}</strong><p>{t("accounts.importIncompleteHint", { count: completed.length })}</p><ul className="import-failure-list">{completed.map((failure) => <li key={failure.itemId}><div><strong>{failure.label || t("accounts.importUnknownAccount")}</strong><code title={t("accounts.importTechnicalCode")}>{failure.code}</code></div>{failure.identity ? <span>{failure.identity}</span> : null}<p>{importFailureReason(failure.code, t)}</p></li>)}</ul></div> : session ? <div className="import-preview"><table className="relay-table"><thead><tr><th><span className="sr-only">{t("accounts.selectImport")}</span></th><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("accounts.identity")}</th><th>{t("accounts.plan")}</th></tr></thead><tbody>{session.preview.rows.map((row) => {
    const badge = row.status === "invalid" ? "error" : row.status === "quota_failed" ? "warning" : row.status === "existing" ? "info" : "ready";
    return <tr key={row.itemId}><td><input type="checkbox" checked={selected.includes(row.itemId)} disabled={!row.selectable} aria-label={t("accounts.selectImportRow", { name: row.label })} onChange={() => toggle(row.itemId)} /></td><td><StatusBadge status={badge} label={t(`accounts.importStatus.${row.status}`, { defaultValue: row.status })} /></td><td>{row.label}{row.error ? <small className="error-text">{t("accounts.importIssue", { code: row.error.code })}</small> : row.warnings.length ? <small>{row.warnings.map((warning) => warning.code).join(", ")}</small> : null}</td><td><code>{row.identity}</code></td><td>{row.plan ?? "-"}</td></tr>;
  })}</tbody></table></div> : <div className="relay-form"><div className="import-file-picker"><Button variant="secondary" icon={<Upload aria-hidden />} busy={busy === "import-files"} onClick={() => chooseFiles()}>{t("accounts.chooseImportFiles")}</Button><span>{t("accounts.importFileHint")}</span></div><label className="relay-field"><span>{t("accounts.importData")}</span><textarea value={content} onChange={(event) => setContent(event.target.value)} placeholder={mode === "local" ? t("accounts.importPlaceholder") : t("accounts.remoteImportPlaceholder")} spellCheck={false} /></label>{mode === "local" ? <details className="import-resume"><summary>{t("accounts.resumeExistingImport")}</summary><label className="relay-field"><span>{t("accounts.resumeImportId")}</span><div className="inline-actions"><input value={resumeId} onChange={(event) => setResumeId(event.target.value)} /><Button variant="secondary" busy={busy === "import-resume"} disabled={!resumeId.trim()} onClick={resume}>{t("common.resume")}</Button></div></label></details> : null}</div>;
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
  return <Dialog wide title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} /></label><div className="form-row"><span>{t("automations.windows")}</span><label><input type="checkbox" checked={windowKinds.includes("primary")} onChange={() => toggleWindow("primary")} />{t("quota.primary")}</label><label><input type="checkbox" checked={windowKinds.includes("secondary")} onChange={() => toggleWindow("secondary")} />{t("quota.secondary")}</label></div><label className="relay-field"><span>{t("automations.accountSelection")}</span><select value={selectorKind} onChange={(event) => setSelectorKind(event.target.value as WakeTask["accountSelector"]["kind"])}><option value="all_eligible">{t("automations.allEligible")}</option><option value="account_ids">{t("automations.selectedAccounts")}</option><option value="tags">{t("automations.matchingTags")}</option></select></label>{selectorKind === "account_ids" ? <fieldset><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} />{account.label}</label>)}</div></fieldset> : null}{selectorKind === "tags" ? <label className="relay-field"><span>{t("automations.tags")}</span><input value={tags} onChange={(event) => setTags(event.target.value)} placeholder={t("automations.tagsPlaceholder")} /></label> : null}<label className="relay-field"><span>{t("automations.modelPolicy")}</span><select value={modelKind} onChange={(event) => setModelKind(event.target.value as WakeTask["modelPolicy"]["kind"])}><option value="lightest_supported">{t("automations.lightest")}</option><option value="explicit">{t("automations.explicitModel")}</option></select></label>{modelKind === "explicit" ? <label className="relay-field"><span>{t("common.model")}</span><select value={modelId} onChange={(event) => setModelId(event.target.value)}><option value="">{t("automations.selectModel")}</option>{availableModels.map((model) => <option key={model} value={model}>{model}</option>)}</select></label> : null}<label className="toggle-row"><input type="checkbox" checked={automatic} onChange={(event) => setAutomatic(event.target.checked)} /><span>{t("automations.automatic")}</span></label><p className="form-note">{t("automations.fixedPrompt")}</p></div></Dialog>;
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
  const { perform, busy } = useRelayState();
  const [apiKey, setApiKey] = useState("");
  const save = async () => { const ok = await perform("ready-save", () => saveKey(apiKey), "feedback.connected"); if (ok) onClose(); };
  return <Dialog title={t("readyApi.connect")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "ready-save"} disabled={!apiKey.trim()} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><div className="recommended-line"><strong>Zenith API</strong><span>{t("common.recommended")}</span></div><SecretField label={t("readyApi.key")} value={apiKey} onChange={setApiKey} /></div></Dialog>;
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

function compareAccounts(
  left: AccountSummary,
  right: AccountSummary,
  sortBy: AccountSort,
  direction: SortDirection,
  planOrder: Map<string, number>,
  unknownPlanRank: number,
) {
  if (sortBy === "pool") {
    return Number(accountRouted(right)) - Number(accountRouted(left))
      || right.priority - left.priority
      || compareOptional(quotaFloor(left), quotaFloor(right), "desc")
      || right.weight - left.weight
      || left.label.localeCompare(right.label);
  }
  if (sortBy === "participation") {
    const comparison = Number(accountParticipates(left)) - Number(accountParticipates(right));
    return (direction === "desc" ? -comparison : comparison)
      || left.label.localeCompare(right.label);
  }
  if (sortBy === "primary" || sortBy === "secondary") {
    return compareOptional(left.quota[sortBy]?.availableBasisPoints ?? null, right.quota[sortBy]?.availableBasisPoints ?? null, direction)
      || left.label.localeCompare(right.label);
  }
  if (sortBy === "primary_reset" || sortBy === "secondary_reset") {
    const kind = sortBy === "primary_reset" ? "primary" : "secondary";
    return compareOptional(left.quota[kind]?.resetAtMs ?? null, right.quota[kind]?.resetAtMs ?? null, direction)
      || left.label.localeCompare(right.label);
  }
  if (sortBy === "plan") {
    const leftRank = planOrder.get(accountPlanOption(left.subscription.planType, "").id) ?? unknownPlanRank;
    const rightRank = planOrder.get(accountPlanOption(right.subscription.planType, "").id) ?? unknownPlanRank;
    return (direction === "desc" ? rightRank - leftRank : leftRank - rightRank)
      || left.label.localeCompare(right.label);
  }
  return (direction === "desc" ? right.label.localeCompare(left.label) : left.label.localeCompare(right.label));
}

function accountParticipates(account: AccountSummary) {
  return account.inPool;
}

function accountRouted(account: AccountSummary) {
  return account.inPool && account.routingExclusion == null;
}

function quotaFloor(account: AccountSummary) {
  const values = [account.quota.primary, account.quota.secondary]
    .map((window) => window?.availableBasisPoints)
    .filter((value): value is number => value != null);
  return values.length ? Math.min(...values) : null;
}

function compareOptional(left: number | null, right: number | null, direction: SortDirection) {
  if (left == null && right == null) return 0;
  if (left == null) return 1;
  if (right == null) return -1;
  return direction === "desc" ? right - left : left - right;
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
