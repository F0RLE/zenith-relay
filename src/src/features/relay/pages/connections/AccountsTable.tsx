import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  CircleAlert,
  CircleCheck,
  CirclePause,
  Coins,
  DollarSign,
  Download,
  Eye,
  EyeOff,
  Layers3,
  ListMinus,
  ListPlus,
  Loader2,
  Network,
  RefreshCw,
  Trash2,
  UserRound,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { accountPlanOption } from "../../accountPlans";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../../api/types";
import {
  refreshAllAccountQuotas,
  type AccountQuotaRefreshReport,
} from "../../accountQuotaRefresh";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import {
  ActionMenu,
  ActionMenuItem,
  Button,
  EmptyState,
  IconButton,
  OptionMenu,
  AccountPlanBadge,
} from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { providerCreditsSummary } from "../../providerCredits";
import { routingOrderPositions } from "../../routingOrder";
import { createRuntimeCandidateLookup } from "../../runtimeCandidateIndex";
import { canAccountJoinPool, updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import { toggleSelectedId } from "../../state/selectionModel";
import { NoResults } from "./connectionHelpers";
import { AccountErrorDialog } from "./AccountErrorDialog";
import { AccountCard } from "./AccountCard";
import { accountMetadataObservation, visibleAccounts } from "../../accountRoutes";
import { useConnectionAccountRemoval } from "./accountRemoval";
import {
  accountCounts,
  accountPlanOptions,
  accountSelectionState,
  activeAccountPlan,
  accountParticipates,
  filterAndSortAccounts,
  visiblePlanCounts as buildVisiblePlanCounts,
  type ParticipationFilter,
} from "./accountTableModel";

const EMPTY_ACCOUNTS: AccountSummary[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];

export function AccountsTable({
  query,
  onQuery,
  onImport,
  onSignIn,
  onReauthenticate,
  onProxy,
  onBulkProxies,
  onExport,
}: {
  query: string;
  onQuery: (queryText: string) => void;
  onImport: () => void;
  onSignIn: () => void;
  onReauthenticate: (account: AccountSummary, automatic?: boolean) => void;
  onProxy: (account: AccountSummary) => void;
  onBulkProxies: (accountIds: string[]) => void;
  onExport: (accountIds: string[]) => void;
}) {
  const { t, i18n } = useTranslation();
  const membershipPending = useRef(false);
  const {
    mode,
    runtime,
    perform,
    refresh,
    busy,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
  } = useRelayState();
  const { remove, openDeleteMenu, permanentDeleteMenu } = useConnectionAccountRemoval();
  const [selected, setSelected] = useState<string[]>([]);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [groupByPlan, setGroupByPlan] = useState(() => localStorage.getItem("relay.accountsGroupByPlan") === "true");
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const allAccounts = useMemo(() => visibleAccounts(runtime?.accounts ?? EMPTY_ACCOUNTS), [runtime?.accounts]);
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const runtimeFor = useMemo(() => createRuntimeCandidateLookup(runtimeOrder), [runtimeOrder]);
  const accountTimestamps = useMemo(() => (runtime?.accounts ?? EMPTY_ACCOUNTS).flatMap((account) => [
    accountMetadataObservation(account, "plan").staleAtMs,
    accountMetadataObservation(account, "quota").staleAtMs,
    account.subscription.activeUntilMs,
    !account.recoveryRequired ? account.credentialDiagnostics?.expiresAtMs : null,
    account.quota.primary?.resetAtMs,
    account.quota.secondary?.resetAtMs,
    ...(account.quota.supplemental ?? []).map((quotaWindow) => quotaWindow.window.resetAtMs),
    ...(account.inPool
      ? (runtimeFor(account.id, "oauth_account")?.modelRetries ?? []).map((retry) => retry.retryAtMs)
      : []),
  ]), [runtime?.accounts, runtimeFor]);
  const nowMs = useRelativeTimeClock(accountTimestamps);
  const unknownPlanLabel = t("common.unknown");
  const plans = useMemo(() => accountPlanOptions(allAccounts, unknownPlanLabel), [allAccounts, unknownPlanLabel]);
  const { errorCount, inPoolCount, disabledCount } = useMemo(() => accountCounts(allAccounts), [allAccounts]);
  const providerCredits = useMemo(() => providerCreditsSummary(allAccounts), [allAccounts]);
  const providerCreditsValue = providerCredits == null
    ? null
    : providerCredits.kind === "unlimited"
      ? "∞"
      : formatNumber(providerCredits.availableCredits, i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 1 });
  const runtimePosition = useMemo(() => routingOrderPositions(runtimeOrder), [runtimeOrder]);
  const runtimeByAccount = useMemo(() => new Map(allAccounts.map((account) => [
    account.id,
    account.inPool ? runtimeFor(account.id, "oauth_account") : undefined,
  ])), [allAccounts, runtimeFor]);
  const activePlan = useMemo(() => activeAccountPlan(planFilter, plans, errorCount), [errorCount, planFilter, plans]);
  useEffect(() => setSelected((previousSelectedAccountIds) => previousSelectedAccountIds.filter((accountId) => allAccounts.some((account) => account.id === accountId))), [runtime?.accounts]);
  useEffect(() => { setSelected([]); setPlanFilter("all"); setParticipationFilter("all"); }, [mode]);
  const canRefreshQuota = mode === "local";
  const accounts = useMemo(() => filterAndSortAccounts(
    allAccounts,
    query,
    activePlan,
    participationFilter,
    groupByPlan,
    runtimePosition,
    unknownPlanLabel,
  ), [activePlan, allAccounts, groupByPlan, participationFilter, query, runtimePosition, unknownPlanLabel]);
  const filtersActive = Boolean(query.trim()) || activePlan !== "all" || participationFilter !== "all";
  const filtersHideAccounts = filtersActive && accounts.length !== allAccounts.length;
  const {
    selectedAccounts,
    selectedCount,
    selectedRecoveryRequired,
    exportIds,
    canIncludeSelected,
    canExcludeSelected,
    allSelected,
  } = useMemo(() => accountSelectionState(allAccounts, accounts, selected), [accounts, allAccounts, selected]);
  const visiblePlanCounts = useMemo(() => buildVisiblePlanCounts(accounts, unknownPlanLabel), [accounts, unknownPlanLabel]);
  const participationOptions = useMemo(() => (["all", "included", "excluded"] as const).map((participationFilterValue) => {
    const count = participationFilterValue === "all" ? allAccounts.length : allAccounts.filter((account) => accountParticipates(account) === (participationFilterValue === "included")).length;
    const participationLabel = t(`accounts.participation.${participationFilterValue}`);
    return { value: participationFilterValue, label: t("accounts.participationFilterOption", { state: participationLabel, count }), shortLabel: `${t("accounts.poolParticipation")}: ${participationLabel}` };
  }), [allAccounts, t]);
  const planFilterOptions = useMemo(() => [
    { value: "all", label: t("accounts.planFilterOption", { plan: t("accounts.allPlans"), count: allAccounts.length }), shortLabel: `${t("accounts.plan")}: ${t("accounts.allPlans")}` },
    ...(errorCount ? [{ value: "errors", label: t("accounts.planFilterOption", { plan: t("accounts.errorsOnly"), count: errorCount }), shortLabel: `${t("accounts.plan")}: ${t("accounts.errorsOnly")}` }] : []),
    ...plans.map((plan) => ({ value: plan.id, label: t("accounts.planFilterOption", { plan: plan.label, count: plan.count }), shortLabel: `${t("accounts.plan")}: ${plan.label}` })),
  ], [allAccounts.length, errorCount, plans, t]);
  if (!runtime?.accounts.length) {
    return (
      <EmptyState
        title={t("accounts.emptyTitle")}
        description={t("accounts.emptyDescription")}
        action={(
          <div className="inline-actions">
            {mode === "local" ? <Button variant="primary" onClick={onSignIn}>{t("accounts.signIn")}</Button> : null}
            <Button
              variant={mode === "local" ? "secondary" : "primary"}
              onClick={onImport}
            >
              {t("accounts.import")}
            </Button>
          </div>
        )}
      />
    );
  }
  const toggleSelected = (accountId: string) => setSelected((selectedIds) => toggleSelectedId(selectedIds, accountId));
  const toggleAllVisible = (checked: boolean) => setSelected(checked ? accounts.map((account) => account.id) : []);
  const togglePlanGrouping = () => setGroupByPlan((previousGroupByPlan) => {
    localStorage.setItem("relay.accountsGroupByPlan", String(!previousGroupByPlan));
    return !previousGroupByPlan;
  });
  const updateSelectedParticipation = async (participate: boolean) => {
    if (membershipPending.current) return;
    membershipPending.current = true;
    const accountsToUpdate = [...selectedAccounts];
    try {
      const accountIds = accountsToUpdate
        .filter((account) => (!participate || canAccountJoinPool(account)) && account.inPool !== participate)
        .map((account) => account.id);
      if (!accountIds.length) return;
      const ok = await perform("pool-membership-bulk", () => updatePoolMembership(mode, { accountIds, sourceIds: [], inPool: participate }), "feedback.saved", { backgroundRefresh: true });
      if (ok) setSelected((previous) => previous.filter((id) => !accountIds.includes(id)));
    } finally {
      membershipPending.current = false;
    }
  };
  const deleteAccounts = async (accountIds: string[], operation: string, permanently: boolean) => {
    const ok = await remove(accountIds, permanently, operation, Boolean(selectedRecoveryRequired));
    if (!ok) await refresh().catch(() => undefined);
    if (ok) setSelected((previousSelectedAccountIds) => previousSelectedAccountIds.filter((accountId) => !accountIds.includes(accountId)));
    return ok;
  };
  const deleteSelected = async (permanently = false) => {
    const accountIds = selectedAccounts.map((account) => account.id);
    if (!accountIds.length) return;
    await deleteAccounts(accountIds, "delete-selected-accounts", permanently);
  };
  const refreshAllQuotas = async () => {
    let report: AccountQuotaRefreshReport | null = null;
    const ok = await perform("quota-all", async () => {
      report = await refreshAllAccountQuotas(mode);
    }, undefined, { backgroundRefresh: true });
    if (ok && report) setQuotaReport(report);
  };
  return (
    <>
    <div className="connections-account-controls workspace-controls">
      <div
        className="connections-account-summary connection-status-summary relay-status-summary"
        data-has-provider-credits={providerCreditsValue != null ? "true" : "false"}
        aria-label={t("accounts.summary.label")}
      >
        <div><UserRound aria-hidden /><strong>{allAccounts.length}</strong><span>{t("accounts.summary.total")}</span></div>
        <div data-tone={inPoolCount ? "ready" : "muted"}><CircleCheck aria-hidden /><strong>{inPoolCount}</strong><span>{t("accounts.summary.inPool")}</span></div>
        <div data-tone={errorCount ? "error" : "muted"}><CircleAlert aria-hidden /><strong>{errorCount}</strong><span>{t("accounts.summary.errors")}</span></div>
        <div data-tone="muted"><CirclePause aria-hidden /><strong>{disabledCount}</strong><span>{t("accounts.summary.disabled")}</span></div>
        {providerCreditsValue != null ? (
          <div data-summary="provider-credits" data-relay-tooltip={t("pool.totalProviderCreditsHint")}>
            <Coins aria-hidden />
            <strong>{providerCreditsValue}</strong>
            <span>{t("pool.totalProviderCredits")}</span>
          </div>
        ) : null}
      </div>
      <div className="account-command-bar" data-selection={selectedCount > 0}>
        <div className="account-command-context">
          <input
            type="checkbox"
            aria-label={t("accounts.selectAll")}
            data-relay-tooltip={t("accounts.selectAll")}
            checked={allSelected}
            disabled={!accounts.length}
            onChange={(event) => toggleAllVisible(event.target.checked)}
          />
          {selectedCount
            ? <span>{t("accounts.selectedCount", { count: selectedCount })}</span>
            : (
              <label className="search-field account-search">
                <span className="sr-only">{t("common.search")}</span>
                <input value={query} onChange={(event) => onQuery(event.target.value)} placeholder={t("common.search")} />
              </label>
            )}
        </div>
        {!selectedCount ? (
          <div className="account-filter-stack">
            <OptionMenu
              className="account-filter-menu"
              label={t("accounts.filterByParticipation")}
              value={participationFilter}
              options={participationOptions}
              onChange={(participationValue) => {
                setSelected([]);
                setParticipationFilter(participationValue as ParticipationFilter);
              }}
            />
            {plans.length > 1 ? (
              <OptionMenu
                className="account-filter-menu"
                label={t("accounts.filterByPlan")}
                value={activePlan}
                options={planFilterOptions}
                onChange={(planId) => {
                  setSelected([]);
                  setPlanFilter(planId);
                }}
              />
            ) : null}
            {allAccounts.length > 1 ? (
              <IconButton
                className="account-group-toggle"
                label={t("accounts.groupByPlan")}
                icon={<Layers3 aria-hidden />}
                aria-pressed={groupByPlan}
                onClick={togglePlanGrouping}
              />
            ) : null}
          </div>
        ) : null}
        <div className="account-command-actions">
          {selectedCount ? <>
            {canIncludeSelected ? (
              <IconButton
                label={t("accounts.includeSelectedInPool")}
                icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListPlus aria-hidden />}
                disabled={busy === "pool-membership-bulk"}
                onClick={() => void updateSelectedParticipation(true)}
              />
            ) : null}
            {canExcludeSelected ? (
              <IconButton
                label={t("accounts.excludeSelectedFromPool")}
                icon={busy === "pool-membership-bulk" ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />}
                disabled={busy === "pool-membership-bulk"}
                onClick={() => void updateSelectedParticipation(false)}
              />
            ) : null}
            <IconButton
              label={t("accounts.exportSelected", { count: selectedCount })}
              icon={<Download aria-hidden />}
              title={t("accounts.exportSelected", { count: selectedCount })}
              onClick={() => onExport(exportIds)}
            />
            <IconButton
              className="danger"
              data-relay-context-action
              label={t("accounts.deleteSelected")}
              icon={busy === "delete-selected-accounts" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />}
              disabled={Boolean(busy)}
              title={t("trash.deleteHint")}
              onClick={() => void deleteSelected()}
              onContextMenu={(event) => openDeleteMenu(event, () => void deleteSelected(true))}
            />
            <IconButton label={t("accounts.clearSelection")} icon={<X aria-hidden />} onClick={() => setSelected([])} />
          </> : <>
            <IconButton
              className="account-calculation-toggle"
              label={t(accountValueVisible ? "pool.hideCalculation" : "pool.showCalculation")}
              icon={<DollarSign aria-hidden />}
              aria-pressed={accountValueVisible}
              onClick={() => setAccountValueVisible(!accountValueVisible)}
            />
            {canRevealAccountIdentities && allAccounts.some((account) => account.secretAvailable) ? (
              <IconButton
                label={t(accountIdentitiesVisible ? "accounts.hideAllIdentities" : "accounts.revealAllIdentities")}
                icon={accountIdentitiesBusy ? <Loader2 className="spin" aria-hidden /> : accountIdentitiesVisible ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                disabled={accountIdentitiesBusy}
                onClick={() => setAccountIdentitiesVisible(!accountIdentitiesVisible)}
              />
            ) : null}
            {canRefreshQuota ? (
              <IconButton
                label={t("accounts.refreshAll")}
                icon={busy === "quota-all" ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
                aria-busy={busy === "quota-all"}
                disabled={busy === "quota-all"}
                onClick={() => void refreshAllQuotas()}
              />
            ) : null}
            <ActionMenu className="account-row-menu account-bulk-menu">
              <ActionMenuItem icon={<Download aria-hidden />} onClick={() => onExport(exportIds)}>
                {t("accounts.exportAll")}
              </ActionMenuItem>
              <ActionMenuItem
                icon={<Network aria-hidden />}
                onClick={() => onBulkProxies(accounts.map((account) => account.id))}
              >
                {t("proxies.assignBulk")}
              </ActionMenuItem>
            </ActionMenu>
          </>}
        </div>
      </div>
    </div>
    {quotaReport ? <AccountQuotaReport report={quotaReport} onClose={() => setQuotaReport(null)} /> : null}
    {filtersHideAccounts ? (
      <div className="account-filter-summary" role="status" aria-live="polite">
        <span>{t("accounts.filterSummary", { visible: accounts.length, total: allAccounts.length })}</span>
        <button
          type="button"
          onClick={() => {
            setSelected([]);
            onQuery("");
            setPlanFilter("all");
            setParticipationFilter("all");
          }}
        >
          <X aria-hidden />
          <span>{t("accounts.clearFilters")}</span>
        </button>
      </div>
    ) : null}
    {accounts.length ? (
      <AccountList
        accounts={accounts}
        groupByPlan={groupByPlan}
        planCounts={visiblePlanCounts}
        nowMs={nowMs}
        selected={selected}
        canRefreshQuota={canRefreshQuota}
        runtimeByAccount={runtimeByAccount}
        onToggleSelected={toggleSelected}
        onShowError={setErrorDetails}
        onProxy={onProxy}
        onExport={onExport}
        onReauthenticate={onReauthenticate}
      />
    ) : <NoResults />}
    {permanentDeleteMenu}
    {errorDetails ? <AccountErrorDialog account={errorDetails} onClose={() => setErrorDetails(null)} /> : null}
    </>
  );
}

function AccountQuotaReport({ report, onClose }: { report: { succeeded: number; failed: number }; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <div className={`account-quota-report${report.failed ? " has-errors" : ""}`} role="status">
      <Check aria-hidden />
      <span>{t("accounts.quotaRefreshReport", report)}</span>
      <button type="button" aria-label={t("common.close")} onClick={onClose}>
        <X aria-hidden />
      </button>
    </div>
  );
}

function AccountList({
  accounts,
  groupByPlan,
  planCounts,
  nowMs,
  selected,
  canRefreshQuota,
  runtimeByAccount,
  onToggleSelected,
  onShowError,
  onProxy,
  onExport,
  onReauthenticate,
}: {
  accounts: AccountSummary[];
  groupByPlan: boolean;
  planCounts: ReadonlyMap<string, number>;
  nowMs: number;
  selected: readonly string[];
  canRefreshQuota: boolean;
  runtimeByAccount: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined>;
  onToggleSelected: (accountId: string) => void;
  onShowError: (account: AccountSummary) => void;
  onProxy: (account: AccountSummary) => void;
  onExport: (accountIds: string[]) => void;
  onReauthenticate: (account: AccountSummary, automatic?: boolean) => void;
}) {
  const { t } = useTranslation();
  const unknownPlan = t("common.unknown");
  return (
    <div className="account-list" role="list" aria-label={t("connections.accounts")}>
      {accounts.map((account, index) => {
        const planType = account.subscription.planType;
        const plan = accountPlanOption(planType, unknownPlan);
        const previousAccount = index ? accounts[index - 1] : undefined;
        const previousPlan = previousAccount ? accountPlanOption(previousAccount.subscription.planType, unknownPlan).id : null;
        return (
          <Fragment key={account.id}>
            {groupByPlan && plan.id !== previousPlan ? (
              <div className="account-plan-group-heading" role="presentation">
                <AccountPlanBadge planType={planType} unknown={unknownPlan} />
                <span>{t("accounts.groupCount", { count: planCounts.get(plan.id) ?? 0 })}</span>
              </div>
            ) : null}
            <AccountCard
              account={account}
              nowMs={nowMs}
              selected={selected.includes(account.id)}
              canRefreshQuota={canRefreshQuota}
              runtimeState={account.inPool ? runtimeByAccount.get(account.id) : undefined}
              onToggleSelected={onToggleSelected}
              onShowError={onShowError}
              onProxy={onProxy}
              onExport={onExport}
              onReauthenticate={onReauthenticate}
            />
          </Fragment>
        );
      })}
    </div>
  );
}
