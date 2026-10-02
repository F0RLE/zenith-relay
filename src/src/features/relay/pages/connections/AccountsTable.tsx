import { Fragment, useEffect, useMemo, useState } from "react";
import {
  Check,
  CircleAlert,
  CircleCheck,
  CirclePause,
  Clock3,
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
  Upload,
  UserRound,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, AccountTransferProgress, CandidateRuntimeSnapshot, ProfileBinding } from "../../api/types";
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
  accountPlanOption,
  useConfirm,
  AccountPlanBadge,
} from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { providerCreditsSummary } from "../../providerCredits";
import { routingOrderPositions, runtimeCandidateForMember } from "../../routingOrder";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults } from "./connectionHelpers";
import { AccountErrorDialog } from "./AccountErrorDialog";
import { AccountCard } from "./AccountCard";
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
  canImport,
  canManageProxies,
  canExport,
  onImport,
  onSignIn,
  onReauthenticate,
  onProxy,
  onBulkProxies,
  onExport,
}: {
  query: string;
  onQuery: (value: string) => void;
  canImport: boolean;
  canManageProxies: boolean;
  canExport: boolean;
  onImport: () => void;
  onSignIn: () => void;
  onReauthenticate: (account: AccountSummary) => void;
  onProxy: (account: AccountSummary) => void;
  onBulkProxies: (accountIds: string[]) => void;
  onExport: (accountIds: string[]) => void;
}) {
  const { t, i18n } = useTranslation();
  const {
    mode,
    runtime,
    perform,
    activateCodexProfile,
    refresh,
    busy,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
  } = useRelayState();
  const confirm = useConfirm();
  const [selected, setSelected] = useState<string[]>([]);
  const [transfer, setTransfer] = useState<{ accountIds: string[]; progress: AccountTransferProgress } | null>(null);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [groupByPlan, setGroupByPlan] = useState(() => localStorage.getItem("relay.accountsGroupByPlan") === "true");
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const allAccounts = runtime?.accounts ?? EMPTY_ACCOUNTS;
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const accountTimestamps = useMemo(() => allAccounts.flatMap((account) => [
    account.subscription.activeUntilMs,
    account.quota.primary?.resetAtMs,
    account.quota.secondary?.resetAtMs,
    ...(account.quota.supplemental ?? []).map((item) => item.window.resetAtMs),
    ...(account.inPool
      ? (runtimeCandidateForMember(account.id, "oauth_account", runtimeOrder)?.modelRetries ?? []).map((retry) => retry.retryAtMs)
      : []),
  ]), [allAccounts, runtimeOrder]);
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
    account.inPool ? runtimeCandidateForMember(account.id, "oauth_account", runtimeOrder) : undefined,
  ])), [allAccounts, runtimeOrder]);
  const activePlan = useMemo(() => activeAccountPlan(planFilter, plans, errorCount), [errorCount, planFilter, plans]);
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
  const canRefreshQuota = mode === "local" || Boolean(runtime?.capabilities.features.includes("quota"));
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
    selectedIds,
    selectedCount,
    selectedAccessOnly,
    selectedSecretsUnavailable,
    selectedOnServer,
    exportIds,
    canIncludeSelected,
    canExcludeSelected,
    allSelected,
  } = useMemo(() => accountSelectionState(allAccounts, accounts, selected), [accounts, allAccounts, selected]);
  const visiblePlanCounts = useMemo(() => buildVisiblePlanCounts(accounts, unknownPlanLabel), [accounts, unknownPlanLabel]);
  const participationOptions = useMemo(() => (["all", "included", "excluded"] as const).map((value) => {
    const count = value === "all" ? allAccounts.length : allAccounts.filter((account) => accountParticipates(account) === (value === "included")).length;
    const state = t(`accounts.participation.${value}`);
    return { value, label: t("accounts.participationFilterOption", { state, count }), shortLabel: `${t("accounts.poolParticipation")}: ${state}` };
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
              disabled={!canImport}
              title={!canImport ? t("remote.capabilityUnavailable") : undefined}
              onClick={onImport}
            >
              {t("accounts.import")}
            </Button>
          </div>
        )}
      />
    );
  }
  const toggleSelected = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const toggleAllVisible = (checked: boolean) => setSelected(checked ? accounts.map((account) => account.id) : []);
  const togglePlanGrouping = () => setGroupByPlan((current) => {
    localStorage.setItem("relay.accountsGroupByPlan", String(!current));
    return !current;
  });
  const updateSelectedParticipation = async (participate: boolean) => {
    const ok = await perform("pool-membership-bulk", async () => {
      const accountIds = selectedAccounts.map((account) => account.id);
      await updatePoolMembership(mode, { accountIds, sourceIds: [], inPool: participate });
    }, "feedback.saved", { backgroundRefresh: true });
    if (ok) setSelected([]);
  };
  const deleteAccounts = async (accountIds: string[], operation: string) => {
    const ok = await perform(operation, async () => {
      if (mode === "local") {
        if (accountIds.length === 1) {
          const accountId = accountIds[0];
          if (accountId) await relayCommands.deleteAccount(accountId);
        }
        else await relayCommands.deleteAccounts(accountIds);
      } else {
        for (const accountId of accountIds) await relayCommands.remoteAction({ type: "delete_account", id: accountId });
      }
    }, "feedback.deleted", { backgroundRefresh: true });
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
    const bindingsLoaded = await perform("move-profile-check", async () => { bindings = await relayCommands.profileBindings(); }, undefined, { backgroundRefresh: true });
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
    setTransfer({ accountIds, progress: { completed: 0, total: accountIds.length, phase: "preparing", ...(accountIds[0] ? { currentAccountId: accountIds[0] } : {}) } });
    const ok = await perform("move-accounts-to-remote", () => relayCommands.moveAccountsToRemote(accountIds), "feedback.accountsMovedToServer", { backgroundRefresh: true });
    setTransfer(null);
    if (!ok && switchedProfile) {
      await relayCommands.restoreDefaultAccountProfile().then(() => refresh()).catch(() => undefined);
    }
    if (ok && switchedProfile) {
      await perform("move-profile-launch", relayCommands.launchManagedCodex, "feedback.launched", { backgroundRefresh: true });
    }
    if (ok) setSelected([]);
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
              onChange={(value) => {
                setSelected([]);
                setParticipationFilter(value as ParticipationFilter);
              }}
            />
            {plans.length > 1 ? (
              <OptionMenu
                className="account-filter-menu"
                label={t("accounts.filterByPlan")}
                value={activePlan}
                options={planFilterOptions}
                onChange={(value) => {
                  setSelected([]);
                  setPlanFilter(value);
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
            {mode === "local" ? (
              <IconButton
                label={t("accounts.moveToServer")}
                icon={busy === "move-accounts-to-remote" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />}
                disabled={busy === "move-accounts-to-remote" || selectedSecretsUnavailable || selectedAccessOnly || selectedOnServer}
                title={selectedOnServer
                  ? t("accounts.moveToServerAlreadyRemote")
                  : selectedAccessOnly
                    ? t("accounts.moveToServerAccessOnlyUnavailable")
                    : selectedSecretsUnavailable
                      ? t("accounts.moveToServerUnavailable")
                      : t("accounts.moveToServer")}
                onClick={() => void moveSelectedToRemote()}
              />
            ) : null}
            <IconButton
              label={t("accounts.exportSelected", { count: selectedCount })}
              icon={<Download aria-hidden />}
              disabled={!canExport}
              title={!canExport ? t("remote.capabilityUnavailable") : t("accounts.exportSelected", { count: selectedCount })}
              onClick={() => onExport(exportIds)}
            />
            <IconButton
              className="danger"
              label={t("accounts.deleteSelected")}
              icon={busy === "delete-selected-accounts" ? <Loader2 className="spin" aria-hidden /> : <Trash2 aria-hidden />}
              disabled={busy === "delete-selected-accounts"}
              onClick={deleteSelected}
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
              <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport} onClick={() => onExport(exportIds)}>
                {t("accounts.exportAll")}
              </ActionMenuItem>
              <ActionMenuItem
                icon={<Network aria-hidden />}
                disabled={!canManageProxies}
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
    {transfer ? <AccountMoveProgress transfer={transfer} accounts={allAccounts} /> : null}
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
        canManageProxies={canManageProxies}
        canExport={canExport}
        canRefreshQuota={canRefreshQuota}
        runtimeByAccount={runtimeByAccount}
        onToggleSelected={toggleSelected}
        onShowError={setErrorDetails}
        onProxy={onProxy}
        onExport={onExport}
        onReauthenticate={onReauthenticate}
      />
    ) : <NoResults />}
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

function AccountMoveProgress({
  transfer,
  accounts,
}: {
  transfer: { accountIds: string[]; progress: AccountTransferProgress };
  accounts: readonly AccountSummary[];
}) {
  const { t } = useTranslation();
  return (
    <div className="account-transfer-progress" role="status" aria-live="polite">
      <header>
        <span><Loader2 className="spin" aria-hidden /></span>
        <div>
          <strong>{t("accounts.moveProgress", { completed: transfer.progress.completed, total: transfer.progress.total })}</strong>
          <small>{t(`accounts.moveProgressPhase.${transfer.progress.phase}`)}</small>
        </div>
        <b>{transfer.progress.completed}/{transfer.progress.total}</b>
      </header>
      <progress max={Math.max(1, transfer.progress.total)} value={transfer.progress.completed} />
      <ul>
        {transfer.accountIds.map((accountId, index) => {
          const account = accounts.find((item) => item.id === accountId);
          const status = index < transfer.progress.completed ? "validated" : index === transfer.progress.completed ? "current" : "pending";
          return (
            <li key={accountId} data-transfer-state={status}>
              {status === "validated" ? <Check aria-hidden /> : status === "current" ? <Loader2 className="spin" aria-hidden /> : <Clock3 aria-hidden />}
              <span>{account?.label ?? accountId}</span>
              <small>{t(`accounts.moveProgressStatus.${status}`)}</small>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function AccountList({
  accounts,
  groupByPlan,
  planCounts,
  nowMs,
  selected,
  canManageProxies,
  canExport,
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
  canManageProxies: boolean;
  canExport: boolean;
  canRefreshQuota: boolean;
  runtimeByAccount: ReadonlyMap<string, CandidateRuntimeSnapshot | undefined>;
  onToggleSelected: (accountId: string) => void;
  onShowError: (account: AccountSummary) => void;
  onProxy: (account: AccountSummary) => void;
  onExport: (accountIds: string[]) => void;
  onReauthenticate: (account: AccountSummary) => void;
}) {
  const { t } = useTranslation();
  const unknownPlan = t("common.unknown");
  return (
    <div className="account-list" role="list" aria-label={t("connections.accounts")}>
      {accounts.map((account, index) => {
        const plan = accountPlanOption(account.subscription.planType, unknownPlan);
        const previousAccount = index ? accounts[index - 1] : undefined;
        const previousPlan = previousAccount ? accountPlanOption(previousAccount.subscription.planType, unknownPlan).id : null;
        return (
          <Fragment key={account.id}>
            {groupByPlan && plan.id !== previousPlan ? (
              <div className="account-plan-group-heading" role="presentation">
                <AccountPlanBadge planType={account.subscription.planType} unknown={unknownPlan} />
                <span>{t("accounts.groupCount", { count: planCounts.get(plan.id) ?? 0 })}</span>
              </div>
            ) : null}
            <AccountCard
              account={account}
              nowMs={nowMs}
              selected={selected.includes(account.id)}
              canManageProxies={canManageProxies}
              canExport={canExport}
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
