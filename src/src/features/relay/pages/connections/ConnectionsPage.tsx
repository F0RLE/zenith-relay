import { ChangeEvent, FormEvent, Fragment, lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { CalendarDays, Check, CircleAlert, Clock3, Copy, Database, Download, ExternalLink, Eye, EyeOff, Layers3, ListMinus, ListPlus, Loader2, LogIn, MapPin, Network, Pencil, Play, Plus, Power, RefreshCw, Server, Shuffle, Square, Trash2, Upload, UserRound, UsersRound, WifiOff, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { defaultWakeInput, relayCommands } from "../../api/commands";
import type { AccountExportFormat, AccountImportProgress, AccountSummary, AccountTransferProgress, ConfirmAccountImportResponse, ImportSession, OAuthFlow, ProfileBinding, ProxyAssignmentResult, ProxyPoolEntry, ProxyPoolSummary, RelayMode, RuntimeSnapshot, SourceSummary, StoredProxyAssignmentResult, WakeTask } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import {
  Button,
  QuotaEconomicsStrip,
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
  SettingToggle,
  StatusBadge,
  StatusIcon,
  Tabs,
  copyText,
  currentAccountErrorCode,
  accountErrorLabel,
  accountPlanOption,
  compareAccountPlans,
  formatDetailedRemainingTime,
  operationalStatusTone,
  useConfirm,
} from "../../components/Ui";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useRelayState } from "../../state/RelayStateProvider";
import { compareRoutingOrder, routingOrderPositions } from "../../routingOrder";
import { SourcePriceEditor, parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "../../components/SourcePriceEditor";

type View = "sources" | "accounts" | "proxies" | "automations" | "remote";
type DialogKind = "source" | "automation" | "remote" | "deploy" | "accountProxy" | "bulkProxies" | "proxyImport" | "oauthSetup" | "accountExport" | null;
type ParticipationFilter = "all" | "included" | "excluded";
type ImportFailure = { itemId: string; code: string; label?: string; identity?: string };
type AccountProxyChoice = "direct" | "automatic" | "stored" | "custom" | "common";
const CONNECTIONS_VIEW_REQUEST = "relay.connections.requestedView";
const MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH = 2_000;
const MarkdownDescription = lazy(() => import("../../components/MarkdownDescription"));

const accountExportFormats: Array<{ value: AccountExportFormat; label: string; multiple: boolean }> = [
  { value: "zenith", label: "Zenith", multiple: true },
  { value: "sub2api", label: "sub2api", multiple: true },
  { value: "cpa", label: "CPA", multiple: false },
  { value: "cockpit", label: "Cockpit Tools", multiple: true },
  { value: "9router", label: "9router", multiple: true },
  { value: "codex", label: "ChatGPT", multiple: false },
  { value: "axon_hub", label: "AxonHub", multiple: false },
  { value: "codex_manager", label: "Codex-Manager", multiple: true },
];

function MarkdownPreview({ content }: { content: string }) {
  return <Suspense fallback={<div className="markdown-loading" role="status"><Loader2 className="spin" aria-hidden /></div>}><MarkdownDescription content={content} /></Suspense>;
}

export function ConnectionsPage({ onImport }: { onImport: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, refresh } = useRelayState();
  const [view, setView] = useState<View>(mode === "zenith" ? "sources" : "accounts");
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
  const startOAuth = () => void oauth.start(false);
  const remoteFeatures = new Set(runtime?.capabilities.features ?? []);
  const supports = (feature: string) => mode !== "remote" || remoteFeatures.has(feature);
  const canImportAccounts = mode !== "remote" || supports("account_batch_import");
  const canManageProxies = supports("account_proxies");
  const canExportAccounts = supports("account_export");
  const showTableToolbar = view === "sources"
    ? Boolean(runtime?.sources.length)
    : view === "automations" && Boolean(runtime?.automations.length);

  useEffect(() => {
    const requested = mode === "zenith" ? null : sessionStorage.getItem(CONNECTIONS_VIEW_REQUEST);
    setView((current) => mode === "zenith" ? "sources" : requested === "sources" || current === "sources" ? "sources" : "accounts");
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
      ? [{ id: "sources", label: t("connections.sources") }]
    : [
        ...(supports("accounts") ? [{ id: "accounts", label: t("connections.accounts") }] : []),
        ...(supports("sources") ? [{ id: "sources", label: t("connections.sources") }] : []),
        ...(mode === "local" ? [{ id: "proxies", label: t("proxies.storage") }] : []),
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
        : runtime ? t("remote.refresh") : t("remote.connect");

  const primaryAction = () => {
    if (view === "accounts" && !canImportAccounts) return;
    if (view === "accounts" && mode === "local") {
      startOAuth();
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
    setEditingSource(null);
    setEditingAutomation(null);
    setDialog(
      view === "sources" ? "source"
          : view === "automations" ? "automation"
            : "remote",
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
      <Tabs value={view} items={tabs} onChange={(id) => { if (id === "sources") sessionStorage.setItem(CONNECTIONS_VIEW_REQUEST, id); else sessionStorage.removeItem(CONNECTIONS_VIEW_REQUEST); setView(id as View); }} label={t("connections.views")} />
      {showTableToolbar ? <div className={`table-toolbar${view === "sources" ? " relay-compact-content" : ""}`}>
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
        {view === "automations" && mode === "local" ? <Button variant="secondary" icon={<Play aria-hidden />} busy={busy === "wake-due"} onClick={() => perform("wake-due", relayCommands.runWakeConfirmations, "feedback.checked")}>{t("automations.runDue")}</Button> : null}
        <Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={refresh}>{t("common.refresh")}</Button>
      </div> : null}

      {view === "sources" ? <SourcesTable query={query} onEdit={(source) => { setEditingSource(source); setDialog("source"); }} /> : null}
      {view === "accounts" ? <AccountsTable query={query} onQuery={setQuery} canImport={canImportAccounts} canManageProxies={canManageProxies} canExport={canExportAccounts} onImport={onImport} onSignIn={startOAuth} onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }} onBulkProxies={(accountIds) => { setBulkProxyAccountIds(accountIds); setDialog("bulkProxies"); }} onExport={(accountIds) => { setExportAccountIds(accountIds); setDialog("accountExport"); }} /> : null}
      {view === "proxies" ? <ProxyStorageView revision={proxyRevision} onImport={() => setDialog("proxyImport")} /> : null}
      {view === "automations" ? <AutomationsTable query={query} onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}
      {view === "remote" ? <RemoteView onConnect={() => setDialog("remote")} onDeploy={() => setDialog("deploy")} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {oauth.flow ? <OAuthDialog flow={oauth.flow} onCancel={oauth.cancel} /> : null}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "remote" ? <RemoteDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "deploy" ? <DeployDialog onClose={() => setDialog(null)} /> : null}
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
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(query, source.name, source.baseUrl, source.wireApi, source.models));
  if (!sources.length) return <NoResults />;
  const localSource = mode !== "remote";
  const updateParticipation = (source: SourceSummary, inPool: boolean) => perform(
    `source-pool-${source.id}`,
    () => localSource
      ? relayCommands.setPoolMembership([], [source.id], inPool)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [source.id], inPool }),
    "feedback.saved",
  );
  const refreshModels = (source: SourceSummary) => perform(
    `source-models-${source.id}`,
    () => localSource
      ? relayCommands.testSource(source.id)
      : relayCommands.remoteAction({ type: "test_source", id: source.id }),
    "feedback.refreshed",
  );
  return (
    <div className="relay-table-wrap relay-compact-content">
      <table className="relay-table source-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.protocol")}</th><th>{t("common.models")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => {
          const launchBusy = busy === `launch-source-${source.id}`;
          const launchDisabled = !localSource || source.wireApi !== "responses" || !source.enabled || !source.secretAvailable || launchBusy;
          const launchTitle = !localSource
            ? t("sources.launchLocalOnly")
            : source.wireApi !== "responses"
              ? t("sources.launchResponsesOnly")
              : !source.enabled || !source.secretAvailable
                ? t("sources.launchUnavailable")
                : t("sources.launch");
          return <tr key={source.id}>
            <td><StatusIcon status={operationalStatusTone(source.operationalStatus)} label={t(`connections.status.${source.operationalStatus}`)} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td>{source.wireApi === "chat_completions" ? "Chat Completions" : "Responses"}</td>
            <td>{source.models.length}</td>
            <td className="row-actions-cell"><div className="row-actions">
              <IconButton label={t("sources.launch")} icon={launchBusy ? <Loader2 className="spin" aria-hidden /> : <Play aria-hidden />} disabled={launchDisabled} title={launchTitle} onClick={() => {
                void activateCodexProfile(`launch-source-${source.id}`, () => relayCommands.launchCodexSource(source.id), true)
                  .then((activated) => { if (activated) localStorage.setItem("relay.directSourceId", source.id); });
              }} />
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <ActionMenu>
                <ActionMenuItem icon={busy === `source-models-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={busy === `source-models-${source.id}`} onClick={() => void refreshModels(source)}>{t("sources.refreshModels")}</ActionMenuItem>
                {mode !== "zenith" ? <ActionMenuItem icon={source.inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `source-pool-${source.id}`} onClick={() => void updateParticipation(source, !source.inPool)}>{t(source.inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}</ActionMenuItem> : null}
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => localSource ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")}>{source.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("sources.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${source.id}`, () => localSource ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
            </div></td>
          </tr>;
        })}</tbody>
      </table>
    </div>
  );
}

function AccountsTable({ query, onQuery, canImport, canManageProxies, canExport, onImport, onSignIn, onProxy, onBulkProxies, onExport }: { query: string; onQuery: (value: string) => void; canImport: boolean; canManageProxies: boolean; canExport: boolean; onImport: () => void; onSignIn: () => void; onProxy: (account: AccountSummary) => void; onBulkProxies: (accountIds: string[]) => void; onExport: (accountIds: string[]) => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, refresh, busy, accountIdentitiesVisible, accountIdentitiesBusy, canRevealAccountIdentities, setAccountIdentitiesVisible } = useRelayState();
  const confirm = useConfirm();
  const [selected, setSelected] = useState<string[]>([]);
  const [transfer, setTransfer] = useState<{ accountIds: string[]; progress: AccountTransferProgress } | null>(null);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [groupByPlan, setGroupByPlan] = useState(() => localStorage.getItem("relay.accountsGroupByPlan") === "true");
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const allAccounts = runtime?.accounts ?? [];
  const [nowMs, setNowMs] = useState(Date.now());
  const subscriptionExpiryFormat = new Intl.DateTimeFormat(i18n.language, { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" });
  useEffect(() => {
    const upcomingTimes = allAccounts.flatMap((account) => [account.subscription.activeUntilMs, account.quota.primary?.resetAtMs, account.quota.secondary?.resetAtMs, ...(account.quota.supplemental ?? []).map((item) => item.window.resetAtMs)]).filter((value): value is number => value != null && value > nowMs);
    if (!upcomingTimes.length) return;
    const urgent = upcomingTimes.some((value) => value - nowMs < 60 * 60_000);
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
  const errorCount = allAccounts.filter(currentAccountErrorCode).length;
  const inPoolCount = allAccounts.filter(accountParticipates).length;
  const disabledCount = allAccounts.filter((account) => !account.enabled).length;
  const storedPosition = new Map(allAccounts.map((account, index) => [account.id, index]));
  const runtimePosition = routingOrderPositions(runtime?.gateway.routingOrder ?? []);
  const activePlan = planFilter === "all" || planOptions.has(planFilter) || (planFilter === "errors" && errorCount > 0) ? planFilter : "all";
  useEffect(() => setSelected((current) => current.filter((id) => allAccounts.some((account) => account.id === id))), [runtime?.accounts]);
  useEffect(() => { setSelected([]); setPlanFilter("all"); setParticipationFilter("all"); }, [mode]);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void relayCommands.onAccountTransferProgress((progress) => setTransfer((current) => current ? { ...current, progress } : null)).then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, []);
  if (!runtime?.accounts.length) {
    return <EmptyState title={t("accounts.emptyTitle")} description={t("accounts.emptyDescription")} action={<div className="inline-actions">{mode === "local" ? <Button variant="primary" onClick={onSignIn}>{t("accounts.signIn")}</Button> : null}<Button variant={mode === "local" ? "secondary" : "primary"} disabled={!canImport} title={!canImport ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>{t("accounts.import")}</Button></div>} />;
  }
  const canRefreshQuota = mode === "local" || runtime.capabilities.features.includes("quota");
  const accounts = [...runtime.accounts]
    .filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType, account.models))
    .filter((account) => activePlan === "all" || (activePlan === "errors" ? Boolean(currentAccountErrorCode(account)) : accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan))
    .filter((account) => participationFilter === "all" || (participationFilter === "included") === accountParticipates(account))
    .sort((left, right) => groupByPlan
      ? compareAccountPlans(accountPlanOption(left.subscription.planType, t("common.unknown")), accountPlanOption(right.subscription.planType, t("common.unknown"))) || compareRoutingOrder(left.id, right.id, runtimePosition, storedPosition)
      : compareRoutingOrder(left.id, right.id, runtimePosition, storedPosition));
  const filtersActive = Boolean(query.trim()) || activePlan !== "all" || participationFilter !== "all";
  const filtersHideAccounts = filtersActive && accounts.length !== allAccounts.length;
  const selectedAccounts = accounts.filter((account) => selected.includes(account.id));
  const selectedIds = selectedAccounts.map((account) => account.id);
  const selectedCount = selectedAccounts.length;
  const selectedAccessOnly = selectedAccounts.some((account) => account.authState.state === "degraded_access_only");
  const selectedSecretsUnavailable = selectedAccounts.some((account) => !account.secretAvailable);
  const selectedOnServer = selectedAccounts.some((account) => Boolean(account.remoteLocation));
  const exportIds = selectedCount ? selectedIds : allAccounts.map((account) => account.id);
  const canIncludeSelected = !selectedOnServer && selectedAccounts.some((account) => !accountParticipates(account));
  const canExcludeSelected = selectedAccounts.some(accountParticipates);
  const allSelected = accounts.length > 0 && accounts.every((account) => selected.includes(account.id));
  const visiblePlanCounts = accounts.reduce((counts, account) => {
    const id = accountPlanOption(account.subscription.planType, t("common.unknown")).id;
    counts.set(id, (counts.get(id) ?? 0) + 1);
    return counts;
  }, new Map<string, number>());
  const participationOptions = (["all", "included", "excluded"] as const).map((value) => {
    const count = value === "all" ? allAccounts.length : allAccounts.filter((account) => accountParticipates(account) === (value === "included")).length;
    const state = t(`accounts.participation.${value}`);
    return { value, label: t("accounts.participationFilterOption", { state, count }), shortLabel: `${t("accounts.poolParticipation")}: ${state}` };
  });
  const planFilterOptions = [
    { value: "all", label: t("accounts.planFilterOption", { plan: t("accounts.allPlans"), count: allAccounts.length }), shortLabel: `${t("accounts.plan")}: ${t("accounts.allPlans")}` },
    ...(errorCount ? [{ value: "errors", label: t("accounts.planFilterOption", { plan: t("accounts.errorsOnly"), count: errorCount }), shortLabel: `${t("accounts.plan")}: ${t("accounts.errorsOnly")}` }] : []),
    ...plans.map((plan) => ({ value: plan.id, label: t("accounts.planFilterOption", { plan: plan.label, count: plan.count }), shortLabel: `${t("accounts.plan")}: ${plan.label}` })),
  ];
  const toggleSelected = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const toggleAllVisible = (checked: boolean) => setSelected(checked ? accounts.map((account) => account.id) : []);
  const togglePlanGrouping = () => setGroupByPlan((current) => {
    localStorage.setItem("relay.accountsGroupByPlan", String(!current));
    return !current;
  });
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
      if (mode === "local") {
        if (accountIds.length === 1) await relayCommands.deleteAccount(accountIds[0]);
        else await relayCommands.deleteAccounts(accountIds);
      } else {
        for (const accountId of accountIds) await relayCommands.remoteAction({ type: "delete_account", id: accountId });
      }
    }, "feedback.deleted");
    if (!ok) await refresh().catch(() => undefined);
    if (ok) setSelected((current) => current.filter((id) => !accountIds.includes(id)));
    return ok;
  };
  const deleteSelected = async () => {
    const accountIds = selectedAccounts.map((account) => account.id);
    const message = mode === "remote"
      ? t("accounts.deleteRemoteSelectedConfirm", { count: accountIds.length })
      : selectedOnServer
        ? t("accounts.deleteSelectedRecoveryConfirm", { count: accountIds.length })
        : t("accounts.deleteSelectedConfirm", { count: accountIds.length });
    if (accountIds.length && await confirm(message, { danger: true })) {
      await deleteAccounts(accountIds, "delete-selected-accounts");
    }
  };
  const moveSelectedToRemote = async () => {
    if (!await confirm(t("accounts.moveToServerConfirm", { count: selectedCount }), {
      title: t("accounts.moveToServer"),
      confirmLabel: t("accounts.moveToServerAction"),
    })) return;
    const accountIds = [...selectedIds];
    let bindings: ProfileBinding[] = [];
    const bindingsLoaded = await perform("move-profile-check", async () => { bindings = await relayCommands.profileBindings(); });
    if (!bindingsLoaded) return;
    const usesSelectedAccount = bindings.some((binding) => binding.active
      && (accountIds.includes(binding.credentialId) || (binding.boundOauthAccountId != null && accountIds.includes(binding.boundOauthAccountId))));
    let switchedProfile = false;
    if (usesSelectedAccount) {
      if (!await confirm(t("accounts.moveActiveProfileConfirm"), {
        title: t("accounts.moveActiveProfileTitle"),
        confirmLabel: t("accounts.moveActiveProfileAction"),
      })) return;
      switchedProfile = await activateCodexProfile("move-profile-switch", relayCommands.attachCodexRemoteGateway);
      if (!switchedProfile) return;
    }
    setTransfer({ accountIds, progress: { completed: 0, total: accountIds.length, phase: "preparing", currentAccountId: accountIds[0] } });
    const ok = await perform("move-accounts-to-remote", () => relayCommands.moveAccountsToRemote(accountIds), "feedback.accountsMovedToServer");
    setTransfer(null);
    if (!ok && switchedProfile) {
      await relayCommands.restoreDefaultAccountProfile().then(() => refresh()).catch(() => undefined);
    }
    if (ok && switchedProfile) {
      await perform("move-profile-launch", relayCommands.launchManagedCodex, "feedback.launched");
    }
    if (ok) setSelected([]);
  };
  const returnToComputer = async (account: AccountSummary) => {
    if (!await confirm(t("accounts.returnToComputerConfirm", { name: account.label }), {
      title: t("accounts.returnToComputer"),
      confirmLabel: t("accounts.returnToComputerAction"),
    })) return;
    await perform(`return-account-${account.id}`, () => relayCommands.returnAccountToLocal(account.id), "feedback.accountReturnedToComputer");
  };
  const recoverLocally = async (account: AccountSummary) => {
    if (!await confirm(t("accounts.forceActivateLocalConfirm", { name: account.label }), {
      title: t("accounts.forceActivateLocal"),
      confirmLabel: t("accounts.forceActivateLocalAction"),
      danger: true,
    })) return;
    await perform(`recover-account-${account.id}`, () => relayCommands.forceActivateRemoteAccountLocally(account.id), "feedback.accountRecoveredLocally");
  };
  const refreshAllQuotas = async () => {
    let results: Awaited<ReturnType<typeof relayCommands.refreshAllAccountQuotas>> = [];
    const ok = await perform("quota-all", async () => {
      results = await relayCommands.refreshAllAccountQuotas();
    });
    if (ok) {
      setQuotaReport({
        succeeded: results.filter((result) => result.status === "succeeded").length,
        failed: results.filter((result) => result.status === "failed").length,
      });
    }
  };
  const refreshAccountQuota = (account: AccountSummary) => perform(
    `connection-account-quota-${account.id}`,
    () => mode === "local"
      ? relayCommands.refreshAccountQuota(account.id)
      : relayCommands.remoteAction({ type: "refresh_account", id: account.id }),
    "feedback.refreshed",
  );
  return (
    <>
    <div className="connections-account-controls">
    <div className="account-command-bar">
      <div className="account-command-context">
        <input type="checkbox" aria-label={t("accounts.selectAll")} title={t("accounts.selectAll")} checked={allSelected} disabled={!accounts.length} onChange={(event) => toggleAllVisible(event.target.checked)} />
        {selectedCount ? <span>{t("accounts.selectedCount", { count: selectedCount })}</span> : <label className="search-field account-search"><span className="sr-only">{t("common.search")}</span><input value={query} onChange={(event) => onQuery(event.target.value)} placeholder={t("common.search")} /></label>}
      </div>
      {!selectedCount ? <div className="account-filter-stack">
        <OptionMenu className="account-filter-menu" label={t("accounts.filterByParticipation")} value={participationFilter} options={participationOptions} onChange={(value) => { setSelected([]); setParticipationFilter(value as ParticipationFilter); }} />
        {plans.length > 1 ? <OptionMenu className="account-filter-menu" label={t("accounts.filterByPlan")} value={activePlan} options={planFilterOptions} onChange={(value) => { setSelected([]); setPlanFilter(value); }} /> : null}
        {allAccounts.length > 1 ? <Button className="account-group-toggle" variant="secondary" icon={<Layers3 aria-hidden />} title={t("accounts.groupByPlan")} aria-pressed={groupByPlan} onClick={togglePlanGrouping}>{t("accounts.groupByPlan")}</Button> : null}
      </div> : null}
      <div className="account-command-actions">
        {selectedCount ? <>
          {canIncludeSelected ? <IconButton label={t("accounts.includeSelectedInPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListPlus aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(true)} /> : null}
          {canExcludeSelected ? <IconButton label={t("accounts.excludeSelectedFromPool")} icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />} disabled={Boolean(busy)} onClick={() => void updateSelectedParticipation(false)} /> : null}
          {mode === "local" ? <IconButton label={t("accounts.moveToServer")} icon={busy === "move-accounts-to-remote" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />} disabled={Boolean(busy) || selectedSecretsUnavailable || selectedAccessOnly || selectedOnServer} title={selectedOnServer ? t("accounts.moveToServerAlreadyRemote") : selectedAccessOnly ? t("accounts.moveToServerAccessOnlyUnavailable") : selectedSecretsUnavailable ? t("accounts.moveToServerUnavailable") : t("accounts.moveToServer")} onClick={() => void moveSelectedToRemote()} /> : null}
          <IconButton label={t("accounts.exportSelected", { count: selectedCount })} icon={<Download aria-hidden />} disabled={!canExport || Boolean(busy)} title={!canExport ? t("remote.capabilityUnavailable") : t("accounts.exportSelected", { count: selectedCount })} onClick={() => onExport(exportIds)} />
          <IconButton className="danger" label={t("accounts.deleteSelected")} icon={busy === "delete-selected-accounts" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={deleteSelected} />
          <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} />
        </> : <>
          {canRevealAccountIdentities && allAccounts.some((account) => account.secretAvailable) ? <IconButton label={t(accountIdentitiesVisible ? "accounts.hideAllIdentities" : "accounts.revealAllIdentities")} icon={accountIdentitiesBusy ? <Loader2 className="spin" aria-hidden /> : accountIdentitiesVisible ? <EyeOff aria-hidden /> : <Eye aria-hidden />} disabled={accountIdentitiesBusy} onClick={() => setAccountIdentitiesVisible(!accountIdentitiesVisible)} /> : null}
          {mode === "local" ? <IconButton label={t("accounts.refreshAll")} icon={busy === "quota-all" ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => void refreshAllQuotas()} /> : null}
          <ActionMenu className="account-row-menu account-bulk-menu">
            <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport} onClick={() => onExport(exportIds)}>{t("accounts.exportAll")}</ActionMenuItem>
            <ActionMenuItem icon={<Network aria-hidden />} disabled={!canManageProxies} onClick={() => onBulkProxies(accounts.map((account) => account.id))}>{t("proxies.assignBulk")}</ActionMenuItem>
          </ActionMenu>
        </>}
      </div>
    </div>
    <div className="connections-account-summary" aria-label={t("accounts.summary.label")}>
      <div><span>{t("accounts.summary.total")}</span><strong>{allAccounts.length}</strong></div>
      <div><span>{t("accounts.summary.inPool")}</span><strong>{inPoolCount}</strong></div>
      <div><span>{t("accounts.summary.errors")}</span><strong>{errorCount}</strong></div>
      <div><span>{t("accounts.summary.disabled")}</span><strong>{disabledCount}</strong></div>
    </div>
    </div>
    {quotaReport ? <div className={`account-quota-report${quotaReport.failed ? " has-errors" : ""}`} role="status"><Check aria-hidden /><span>{t("accounts.quotaRefreshReport", quotaReport)}</span><button type="button" aria-label={t("common.close")} onClick={() => setQuotaReport(null)}><X aria-hidden /></button></div> : null}
    {transfer ? <div className="account-transfer-progress" role="status" aria-live="polite">
      <header><span><Loader2 className="spin" aria-hidden /></span><div><strong>{t("accounts.moveProgress", { completed: transfer.progress.completed, total: transfer.progress.total })}</strong><small>{t(`accounts.moveProgressPhase.${transfer.progress.phase}`)}</small></div><b>{transfer.progress.completed}/{transfer.progress.total}</b></header>
      <progress max={Math.max(1, transfer.progress.total)} value={transfer.progress.completed} />
      <ul>{transfer.accountIds.map((accountId, index) => {
        const account = allAccounts.find((item) => item.id === accountId);
        const status = index < transfer.progress.completed ? "validated" : index === transfer.progress.completed ? "current" : "pending";
        return <li key={accountId} data-transfer-state={status}>{status === "validated" ? <Check aria-hidden /> : status === "current" ? <Loader2 className="spin" aria-hidden /> : <Clock3 aria-hidden />}<span>{account?.label ?? accountId}</span><small>{t(`accounts.moveProgressStatus.${status}`)}</small></li>;
      })}</ul>
    </div> : null}
    {filtersHideAccounts ? <div className="account-filter-summary" role="status" aria-live="polite"><span>{t("accounts.filterSummary", { visible: accounts.length, total: allAccounts.length })}</span><button type="button" onClick={() => { setSelected([]); onQuery(""); setPlanFilter("all"); setParticipationFilter("all"); }}><X aria-hidden /><span>{t("accounts.clearFilters")}</span></button></div> : null}
    {accounts.length ? <div className="account-list" role="list" aria-label={t("connections.accounts")}>
      {accounts.map((account, index) => {
        const plan = accountPlanOption(account.subscription.planType, t("common.unknown"));
        const previousPlan = index ? accountPlanOption(accounts[index - 1].subscription.planType, t("common.unknown")).id : null;
        const participates = accountParticipates(account);
        const subscriptionEnded = account.subscription.activeUntilMs != null && account.subscription.activeUntilMs <= Date.now();
        const subscriptionEnd = account.subscription.activeUntilMs == null
          ? { date: t("accounts.subscriptionEndUnknown"), relative: null }
          : { date: subscriptionExpiryFormat.format(account.subscription.activeUntilMs), relative: formatDetailedRemainingTime(account.subscription.activeUntilMs, nowMs, t) };
        const onServer = mode === "local" && Boolean(account.remoteLocation);
        const errorCode = onServer
          ? account.lastErrorCode === "remote_missing" ? "remote_missing" : null
          : currentAccountErrorCode(account);
        const remoteMissing = onServer && errorCode === "remote_missing";
        const operationalStatus = account.operationalStatus;
        const operationalLabel = remoteMissing ? accountErrorLabel(errorCode, t) : onServer ? t("accounts.onServerHint") : t(`connections.status.${operationalStatus}`);
        const proxyLabel = account.proxyAvailable === false && account.proxyMode === "direct" ? t("proxies.modes.blocked") : t(`proxies.modes.${account.proxyMode ?? "direct"}`);
        const poolLabel = participates ? t("accounts.participation.included") : t("accounts.participation.excluded");
        const quotaStatus = account.quotaRefreshStatus;
        const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
        const indicatorTone = quotaStatus === "refreshing" ? "disabled" : quotaStatus === "failed" || quotaStatus === "requires_reauth" ? "error" : quotaStatus === "pending" ? "disabled" : onServer ? "info" : operationalStatusTone(operationalStatus);
        const indicatorLabel = quotaStatus === "updated" ? operationalLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${operationalLabel}`;
        const selectedAccount = selected.includes(account.id);
        return <Fragment key={account.id}>
        {groupByPlan && plan.id !== previousPlan ? <div className="account-plan-group-heading" role="presentation"><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /><span>{t("accounts.groupCount", { count: visiblePlanCounts.get(plan.id) ?? 0 })}</span></div> : null}
        <article className={`account-card${selectedAccount ? " selected" : ""}`} role="listitem">
          <div className="account-card-main">
            {displayedErrorCode
              ? <IconButton className="account-kind-icon account-status-button" data-status="error" label={accountErrorLabel(displayedErrorCode, t)} icon={<UserRound aria-hidden />} onClick={() => setErrorDetails(account)} />
              : <StatusIcon className="account-kind-icon" status={indicatorTone} label={indicatorLabel}><UserRound aria-hidden /></StatusIcon>}
            <div className="account-identity">
              <strong title={account.label}>{account.label}</strong>
              <div className="account-identity-meta"><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></div>
            </div>
            <div className="account-card-header-actions">
              <ActionMenu className="account-row-menu">
                {errorCode ? <ActionMenuItem icon={<CircleAlert aria-hidden />} onClick={() => setErrorDetails(account)}>{t("accounts.errorDetailsTitle")}</ActionMenuItem> : null}
                {onServer ? <ActionMenuItem icon={<Download aria-hidden />} disabled={Boolean(busy)} onClick={() => void returnToComputer(account)}>{t("accounts.returnToComputer")}</ActionMenuItem> : null}
                {onServer ? <ActionMenuItem danger icon={<Power aria-hidden />} disabled={Boolean(busy)} onClick={() => void recoverLocally(account)}>{t("accounts.forceActivateLocal")}</ActionMenuItem> : null}
                <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport || !account.secretAvailable} onClick={() => onExport([account.id])}>{t("accounts.exportOne", { name: account.label })}</ActionMenuItem>
                {!onServer ? <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void perform(`enable-${account.id}`, () => mode === "local" ? relayCommands.setAccountEnabled(account.id, !account.enabled) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled: !account.enabled }), "feedback.saved"); }}>{account.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem> : null}
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t(onServer ? "accounts.deleteLocalRecoveryConfirm" : mode === "remote" ? "accounts.deleteRemoteConfirm" : "accounts.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${account.id}`, () => mode === "local" ? relayCommands.deleteAccount(account.id) : relayCommands.remoteAction({ type: "delete_account", id: account.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
              <IconButton className="account-select-button" label={selectedAccount ? t("accounts.deselect", { name: account.label }) : t("accounts.select", { name: account.label })} icon={selectedAccount ? <Check aria-hidden /> : <Square aria-hidden />} aria-pressed={selectedAccount} onClick={() => toggleSelected(account.id)} />
            </div>
          </div>
          <div className="account-card-quota compact-quota-layout">{accountHasQuotaWindows(account) ? <QuotaStack snapshot={account.quota} nowMs={nowMs} concise /> : <AccountQuotaRefreshState account={account} />}</div>
          <div className={`account-subscription-line${subscriptionEnded ? " expired" : ""}`} title={[subscriptionEnd.date, subscriptionEnd.relative].filter(Boolean).join(" · ")}><CalendarDays aria-hidden /><span>{subscriptionEnd.date}</span>{subscriptionEnd.relative ? <><span className="account-subscription-separator" aria-hidden>·</span><span className="account-subscription-countdown">{subscriptionEnd.relative}</span></> : null}</div>
          <QuotaEconomicsStrip account={account} />
          <footer className="account-card-footer"><div className="account-card-actions">
            {onServer
              ? <IconButton label={t("accounts.onServerHint")} icon={<Server aria-hidden />} disabled />
              : <IconButton className={participates ? "danger" : ""} label={poolLabel} icon={participates ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `pool-${account.id}`} onClick={() => void perform(`pool-${account.id}`, () => updateParticipation(account, !participates), "feedback.saved")} />}
            <IconButton label={t("accounts.refreshQuota")} icon={busy === `connection-account-quota-${account.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={!canRefreshQuota || !account.secretAvailable || Boolean(busy)} onClick={() => void refreshAccountQuota(account)} />
            <IconButton label={`${t("proxies.proxy")}: ${proxyLabel}`} icon={<Pencil aria-hidden />} disabled={onServer || !canManageProxies} onClick={() => onProxy(account)} />
            {mode === "local" ? <IconButton label={t("accounts.launchAccount")} icon={<Play aria-hidden />} disabled={onServer || !account.secretAvailable || busy === `launch-account-${account.id}`} title={onServer ? t("accounts.onServerHint") : !account.secretAvailable ? t("accounts.credentialsUnavailable") : t("accounts.launchAccount")} onClick={() => void activateCodexProfile(`launch-account-${account.id}`, () => relayCommands.launchCodexAccount(account.id), true)} /> : null}
          </div></footer>
        </article>
        </Fragment>;
      })}
    </div> : <NoResults />}
    {errorDetails ? <AccountErrorDialog account={errorDetails} onClose={() => setErrorDetails(null)} /> : null}
    </>
  );
}

function AccountQuotaRefreshState({ account }: { account: AccountSummary }) {
  const { t } = useTranslation();
  const status = account.quotaRefreshStatus ?? (account.authState.state === "requires_reauth"
    ? "requires_reauth"
    : account.quota.error
      ? "failed"
      : account.quota.updatedAtMs != null
        ? "updated"
        : "pending");
  const icon = status === "refreshing"
    ? <Loader2 className="spin" aria-hidden />
    : status === "updated"
      ? <Check aria-hidden />
      : status === "requires_reauth"
        ? <LogIn aria-hidden />
        : status === "failed"
          ? <RefreshCw aria-hidden />
          : <Clock3 aria-hidden />;
  return <div className={`account-quota-refresh-state ${status}`} role="status">{icon}<span>{t(`accounts.quotaRefreshStatus.${status}`)}</span></div>;
}

function accountHasQuotaWindows(account: AccountSummary) {
  return Boolean(account.quota.primary || account.quota.secondary || account.quota.supplemental?.length);
}

export function AccountErrorDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const code = currentAccountErrorCode(account) ?? "unknown";
  const authState = account.authState.state;
  const observedAtMs = account.quota.error?.occurredAtMs ?? null;
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
  const [format, setFormat] = useState<AccountExportFormat>("zenith");
  const [description, setDescription] = useState("");
  const [descriptionMode, setDescriptionMode] = useState<"edit" | "preview">("edit");
  const [descriptionError, setDescriptionError] = useState<string | null>(null);
  const markdownFileInput = useRef<HTMLInputElement>(null);
  const formats = accountExportFormats.filter((option) => accountIds.length === 1 || option.multiple);
  const selectedFormat = formats.find((option) => option.value === format) ?? formats[0];
  const loadMarkdown = async (event: ChangeEvent<HTMLInputElement>) => {
    const input = event.currentTarget;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      const content = (await file.text()).replace(/\r\n?/g, "\n");
      if (content.length > MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH) {
        setDescriptionError(t("accounts.exportDescriptionTooLong", { max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH }));
        return;
      }
      setDescription(content);
      setDescriptionError(null);
      setDescriptionMode("preview");
    } catch {
      setDescriptionError(t("accounts.exportDescriptionReadFailed"));
    }
  };
  const run = async (destination: "copy" | "download") => {
    const ok = await perform(`account-export-${destination}`, async () => {
      const input = {
        accountIds,
        format,
        destination,
        ...(format === "zenith" && description.trim() ? { description } : {}),
      } as const;
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
  return <Dialog title={t("accounts.exportTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="secondary" icon={<Copy aria-hidden />} busy={busy === "account-export-copy"} onClick={() => run("copy")}>{t("accounts.copyExport")}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={busy === "account-export-download"} onClick={() => run("download")}>{t("accounts.downloadExport")}</Button></>}>
    <div className="relay-form account-export-form">
      <div className="account-export-heading"><span>{t("accounts.exportFormat")}</span><strong>{t("accounts.exportCount", { count: accountIds.length })}</strong></div>
      <div className="account-export-formats" data-count={formats.length} role="radiogroup" aria-label={t("accounts.exportFormat")}>{formats.map((option) => <button type="button" role="radio" data-value={option.value} aria-checked={format === option.value} key={option.value} onClick={() => setFormat(option.value)}><span>{option.label}</span>{format === option.value ? <Check aria-hidden /> : null}</button>)}</div>
      <p className="account-export-description">{t(`accounts.exportFormats.${selectedFormat.value}`)}</p>
      {format === "zenith" ? <div className="relay-field account-export-description-field">
        <div className="account-export-description-toolbar">
          <label htmlFor="zenith-export-description">{t("accounts.exportDescription")}</label>
          <div className="account-export-description-controls">
            <input ref={markdownFileInput} type="file" accept=".md,text/markdown" onChange={(event) => void loadMarkdown(event)} />
            <IconButton label={t("accounts.loadMarkdown")} icon={<Upload aria-hidden />} onClick={() => markdownFileInput.current?.click()} />
            <div className="markdown-mode-switch" role="group" aria-label={t("accounts.descriptionMode")}>
              <button type="button" aria-pressed={descriptionMode === "edit"} onClick={() => setDescriptionMode("edit")}><Pencil aria-hidden />{t("accounts.descriptionEdit")}</button>
              <button type="button" aria-pressed={descriptionMode === "preview"} onClick={() => setDescriptionMode("preview")}><Eye aria-hidden />{t("accounts.descriptionPreview")}</button>
            </div>
          </div>
        </div>
        {descriptionMode === "edit"
          ? <textarea id="zenith-export-description" value={description} maxLength={MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH} placeholder={t("accounts.exportDescriptionPlaceholder")} onChange={(event) => { setDescription(event.target.value); setDescriptionError(null); }} />
          : <div className="account-export-markdown-preview" role="region" aria-label={t("accounts.descriptionPreview")}>{description.trim() ? <MarkdownPreview content={description} /> : <p>{t("accounts.descriptionPreviewEmpty")}</p>}</div>}
        {descriptionError ? <p className="form-note error-text" role="alert">{descriptionError}</p> : null}
        <small>{t("accounts.exportDescriptionHint", { count: description.length, max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH })}</small>
      </div> : null}
    </div>
  </Dialog>;
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

function ProxyRouteOption({ value, selected, disabled = false, icon, label, hint, onSelect }: { value: AccountProxyChoice; selected: boolean; disabled?: boolean; icon: React.ReactNode; label: string; hint: string; onSelect: (value: AccountProxyChoice) => void }) {
  return <button type="button" role="radio" aria-checked={selected} disabled={disabled} className={selected ? "selected" : ""} onClick={() => onSelect(value)}>{icon}<span><strong>{label}</strong><small>{hint}</small></span></button>;
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
                <td className="row-actions-cell"><div className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem></ActionMenu></div></td>
              </tr>
            );
          })}</tbody>
        </table>
    </div>
  );
}

function RemoteView({ onConnect, onDeploy }: { onConnect: () => void; onDeploy: () => void }) {
  const { t } = useTranslation();
  const { runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  const disconnect = async () => {
    let linkedAccounts = 0;
    const counted = await perform("remote-disconnect-check", async () => { linkedAccounts = await relayCommands.remoteLinkedAccountCount(); });
    if (!counted) return;
    const message = linkedAccounts
      ? t("remote.disconnectLinkedConfirm", { count: linkedAccounts })
      : t("remote.disconnectConfirm");
    if (await confirm(message, { danger: true })) {
      await perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected");
    }
  };
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.runtimeTarget.connected ? "ready" : "error"} label={runtime.runtimeTarget.connected ? t("common.connected") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="danger" busy={busy === "remote-disconnect-check" || busy === "remote-disconnect"} onClick={() => void disconnect()}>{t("remote.disconnect")}</Button></div></section>;
}

export function SourceDialog({ source, onClose, addToPool = false }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [wireApi, setWireApi] = useState<SourceSummary["wireApi"]>(source?.wireApi ?? "responses");
  const [priceDrafts, setPriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(source?.modelPriceOverrides ?? {}));
  const modelPriceOverrides = useMemo(() => parseSourcePriceDrafts(priceDrafts), [priceDrafts]);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (source && !modelPriceOverrides) return;
    const ok = await perform("source-save", async () => {
      if (!source) {
        const payload = apiProviderSourceInput(provider);
        const created = mode !== "remote"
          ? await relayCommands.createSource(payload) as { id: string }
          : await relayCommands.remoteAction({ type: "create_source" }, payload) as { id: string };
        if (addToPool) {
          if (mode !== "remote") await relayCommands.setPoolMembership([], [created.id], true);
          else await relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [created.id], inPool: true });
        }
        return;
      }
      const update = { name, baseUrl, wireApi, models: source.models, allowedModels: source.allowedModels, excludedModels: source.excludedModels, draining: source.draining, priority: source.priority, weight: source.weight, recoveryDelaySeconds: source.recoveryDelaySeconds, modelPriceOverrides };
      if (mode !== "remote") {
        await relayCommands.updateSource({ sourceId: source.id, ...update });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...update, ...(apiKey ? { apiKey } : {}) });
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={(!source && !apiProviderReady(provider)) || !modelPriceOverrides} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>}><form id="source-form" className="relay-form source-form" onSubmit={submit}>{source ? <><section className="source-form-section"><header><h3>{t("sources.connection")}</h3></header><div className="source-identity-grid"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label></div><div className="source-access-grid"><div className="relay-field"><span>{t("sources.protocol")}</span><OptionMenu className="field-option-menu" label={t("sources.protocol")} value={wireApi} onChange={(value) => setWireApi(value as SourceSummary["wireApi"])} options={[{ value: "responses", label: "Responses API" }, { value: "chat_completions", label: "Chat Completions" }]} /></div><SecretField label={t("sources.replaceKey")} value={apiKey} onChange={setApiKey} /></div></section><SourcePriceEditor source={source} drafts={priceDrafts} onChange={setPriceDrafts} /></> : <ApiProviderForm value={provider} onChange={setProvider} />}</form></Dialog>;
}

function OAuthDialog({ flow, onCancel }: { flow: OAuthFlow; onCancel: () => Promise<void> }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [now, setNow] = useState(Date.now);
  const [reopenAt, setReopenAt] = useState(0);
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
        <Button variant="primary" icon={<ExternalLink aria-hidden />} busy={busy === "oauth-reopen"} disabled={flowUnavailable || reopenIn > 0} onClick={() => void reopen()}>{reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}</Button>
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
  const account = runtime?.accounts.find((item) => item.id === accountId);
  const apply = async () => {
    if (!addToPool && !assignProxy) {
      onClose();
      return;
    }
    const ok = await perform("oauth-setup", async () => {
      if (addToPool) await relayCommands.setPoolMembership([accountId], [], true);
      if (assignProxy) await relayCommands.assignAutomaticProxies([accountId]);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.accountAdded")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("accounts.configureLater")}</Button><Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}>{t("common.done")}</Button></>}><div className="relay-form oauth-account-setup"><div className="oauth-account-added"><Check aria-hidden /><div><strong>{account?.identityHint ?? t("accounts.accountReady")}</strong><p>{t("accounts.accountAddedHint")}</p></div></div><div className="post-import-options"><label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addAccountToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label><label><input type="checkbox" checked={assignProxy} disabled={!pool || pool.total === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignStoredAfterAdd")}</strong><small>{pool ? t(pool.total ? "proxies.storedAvailable" : "proxies.noStored", { count: pool.total }) : t("common.loading")}</small></span></label></div></div></Dialog>;
}

function formatCountdown(seconds: number) {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}

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
  return <Dialog title={t("remote.connectExisting")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "remote-connect"} disabled={!baseUrl || !token || (insecure && !allowInsecure)} onClick={connect}>{t("remote.testAndConnect")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={token} onChange={setToken} /><div className="remote-connect-options">{insecure ? <SettingToggle tone="warning" label={t("remote.allowInsecure")} description={t("remote.allowInsecureHint")} checked={allowInsecure} onChange={setAllowInsecure} /> : null}<SettingToggle label={t("remote.confirmIdentityChange")} description={t("remote.identityHint")} checked={confirmIdentityChange} onChange={setConfirmIdentityChange} /></div></div></Dialog>;
}

function DeployDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [url, setUrl] = useState("");
  const [plan, setPlan] = useState<{ directory: string; managementToken: string; vaultKey: string; composeCommand: string } | null>(null);
  const generate = async () => { const result: { current: typeof plan } = { current: null }; const ok = await perform("remote-deploy", async () => { result.current = await relayCommands.prepareRemoteDeployment(url); }, "feedback.deploymentPrepared"); if (ok) setPlan(result.current); };
  return <Dialog title={t("remote.deployNew")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>{!plan ? <Button variant="primary" busy={busy === "remote-deploy"} disabled={!url} onClick={generate}>{t("remote.generate")}</Button> : null}</>}>{plan ? <div className="deployment-result"><StatusBadge status="ready" label={t("common.ready")} /><label><span>{t("remote.bundlePath")}</span><code>{plan.directory}</code></label><div className="relay-field"><span>{t("remote.token")}</span><div className="endpoint-line"><input aria-label={t("remote.token")} type="password" value={plan.managementToken} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.managementToken)}>{t("common.copy")}</Button></div></div><div className="relay-field"><span>{t("remote.vaultKey")}</span><div className="endpoint-line"><input aria-label={t("remote.vaultKey")} type="password" value={plan.vaultKey} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.vaultKey)}>{t("common.copy")}</Button></div></div><label><span>{t("remote.command")}</span><code>{plan.composeCommand}</code></label><p>{t("remote.secretOnce")}</p><p>{t("remote.deployHint")}</p></div> : <label className="relay-field"><span>{t("remote.publicUrl")}</span><input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://relay.example.com" /></label>}</Dialog>;
}

function safeHost(value: string) {
  try { return new URL(value).host; } catch { return value; }
}

function accountParticipates(account: AccountSummary) {
  return account.inPool;
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
