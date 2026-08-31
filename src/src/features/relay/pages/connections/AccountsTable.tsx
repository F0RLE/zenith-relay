import { Fragment, useEffect, useState } from "react";
import {
  CalendarDays,
  Check,
  CircleAlert,
  Clock3,
  Copy,
  DollarSign,
  Download,
  Eye,
  EyeOff,
  Layers3,
  ListMinus,
  ListPlus,
  Loader2,
  LogIn,
  Network,
  Pencil,
  Play,
  Power,
  RefreshCw,
  Server,
  Square,
  Trash2,
  Upload,
  UserRound,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, AccountTransferProgress, ProfileBinding } from "../../api/types";
import { accountQuotaRefreshState, currentAccountErrorCode, operationalStatusTone, requiresAccountReauthentication, transientCandidateTone } from "../../accountStatus";
import {
  refreshAllAccountQuotas,
  refreshOneAccountQuota,
  type AccountQuotaRefreshReport,
} from "../../accountQuotaRefresh";
import { subscriptionExpiryFormatter, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import {
  AccountPlanBadge,
  ActionMenu,
  ActionMenuItem,
  Button,
  Dialog,
  EmptyState,
  IconButton,
  OptionMenu,
  QuotaStack,
  StatusIcon,
  accountErrorLabel,
  accountPlanOption,
  copyText,
  useConfirm,
} from "../../components/Ui";
import { AccountValueStrip } from "../../components/AccountValueStrip";
import { ResetCreditsControl } from "../../components/ResetCreditsControl";
import { formatDetailedRemainingTime, isFastSupplementalQuota } from "../../quotaFormatting";
import { routingOrderPositions, runtimeCandidateForMember, upcomingModelRetries } from "../../routingOrder";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults } from "./connectionHelpers";
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

export function AccountsTable({ query, onQuery, canImport, canManageProxies, canExport, onImport, onSignIn, onReauthenticate, onProxy, onBulkProxies, onExport }: { query: string; onQuery: (value: string) => void; canImport: boolean; canManageProxies: boolean; canExport: boolean; onImport: () => void; onSignIn: () => void; onReauthenticate: (account: AccountSummary) => void; onProxy: (account: AccountSummary) => void; onBulkProxies: (accountIds: string[]) => void; onExport: (accountIds: string[]) => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, refresh, busy, accountIdentitiesVisible, accountIdentitiesBusy, canRevealAccountIdentities, setAccountIdentitiesVisible, accountValueVisible, setAccountValueVisible } = useRelayState();
  const confirm = useConfirm();
  const [selected, setSelected] = useState<string[]>([]);
  const [transfer, setTransfer] = useState<{ accountIds: string[]; progress: AccountTransferProgress } | null>(null);
  const [planFilter, setPlanFilter] = useState("all");
  const [participationFilter, setParticipationFilter] = useState<ParticipationFilter>("all");
  const [groupByPlan, setGroupByPlan] = useState(() => localStorage.getItem("relay.accountsGroupByPlan") === "true");
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const allAccounts = runtime?.accounts ?? [];
  const runtimeOrder = runtime?.gateway.routingOrder ?? [];
  const nowMs = useRelativeTimeClock(allAccounts.flatMap((account) => [
    account.subscription.activeUntilMs,
    account.quota.primary?.resetAtMs,
    account.quota.secondary?.resetAtMs,
    ...(account.quota.supplemental ?? []).map((item) => item.window.resetAtMs),
    ...(account.inPool
      ? (runtimeCandidateForMember(account.id, "oauth_account", runtimeOrder)?.modelRetries ?? []).map((retry) => retry.retryAtMs)
      : []),
  ]));
  const subscriptionExpiryFormat = subscriptionExpiryFormatter(i18n.language);
  const plans = accountPlanOptions(allAccounts, t("common.unknown"));
  const { errorCount, inPoolCount, disabledCount } = accountCounts(allAccounts);
  const runtimePosition = routingOrderPositions(runtime?.gateway.routingOrder ?? []);
  const runtimeByAccount = new Map(allAccounts.map((account) => [
    account.id,
    account.inPool ? runtimeCandidateForMember(account.id, "oauth_account", runtimeOrder) : undefined,
  ]));
  const activePlan = activeAccountPlan(planFilter, plans, errorCount);
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
  const accounts = filterAndSortAccounts(runtime.accounts, query, activePlan, participationFilter, groupByPlan, runtimePosition, t("common.unknown"));
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
  } = accountSelectionState(allAccounts, accounts, selected);
  const visiblePlanCounts = buildVisiblePlanCounts(accounts, t("common.unknown"));
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
    await updatePoolMembership(mode, { accountIds: [account.id], sourceIds: [], inPool: participate });
  };
  const updateSelectedParticipation = async (participate: boolean) => {
    const ok = await perform("pool-membership-bulk", async () => {
      const accountIds = selectedAccounts.map((account) => account.id);
      await updatePoolMembership(mode, { accountIds, sourceIds: [], inPool: participate });
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
    let report: AccountQuotaRefreshReport | null = null;
    const ok = await perform("quota-all", async () => {
      report = await refreshAllAccountQuotas(mode);
    });
    if (ok && report) setQuotaReport(report);
  };
  const refreshAccountQuota = (account: AccountSummary) => perform(
    `connection-account-quota-${account.id}`,
    () => refreshOneAccountQuota(mode, account.id),
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
        {allAccounts.length > 1 ? <Button className="account-group-toggle" variant="secondary" icon={<Layers3 aria-hidden />} title={t("accounts.groupByPlan")} aria-label={t("accounts.groupByPlan")} aria-pressed={groupByPlan} onClick={togglePlanGrouping}>{t("accounts.groupByPlan")}</Button> : null}
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
          <IconButton className="account-calculation-toggle" label={t(accountValueVisible ? "pool.hideCalculation" : "pool.showCalculation")} icon={<DollarSign aria-hidden />} aria-pressed={accountValueVisible} onClick={() => setAccountValueVisible(!accountValueVisible)} />
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
      <div><span>{t("accounts.summary.total")}</span><i aria-hidden="true">—</i><strong>{allAccounts.length}</strong></div>
      <div><span>{t("accounts.summary.inPool")}</span><i aria-hidden="true">—</i><strong>{inPoolCount}</strong></div>
      <div><span>{t("accounts.summary.errors")}</span><i aria-hidden="true">—</i><strong>{errorCount}</strong></div>
      <div><span>{t("accounts.summary.disabled")}</span><i aria-hidden="true">—</i><strong>{disabledCount}</strong></div>
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
        const runtimeState = account.inPool ? runtimeByAccount.get(account.id) : undefined;
        const modelRetries = upcomingModelRetries(runtimeState, nowMs);
        const modelRetryHint = modelRetries.length
          ? t("pool.modelRetryAt", {
            models: modelRetries.map((retry) => retry.model).join(", "),
            time: formatDetailedRemainingTime(modelRetries[0].retryAtMs, nowMs, t),
          })
          : null;
        const runtimeTone = operationalStatus === "rotation"
          ? transientCandidateTone(runtimeState, nowMs, false) ?? (modelRetries.length ? "warning" : null)
          : null;
        const runtimeHint = runtimeState?.halfOpen
          ? t("pool.recoveryProbe")
          : modelRetryHint;
        const proxyLabel = account.proxyAvailable === false && account.proxyMode === "direct" ? t("proxies.modes.blocked") : t(`proxies.modes.${account.proxyMode ?? "direct"}`);
        const poolActionLabel = participates ? t("accounts.excludeFromPool") : t("accounts.includeInPool");
        const quotaStatus = accountQuotaRefreshState(account);
        const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
        const indicatorTone = onServer
          ? "info"
          : operationalStatus === "unavailable" || operationalStatus === "disabled"
            ? operationalStatusTone(operationalStatus)
            : quotaStatus === "refreshing"
            ? "disabled"
            : quotaStatus === "failed" || quotaStatus === "requires_reauth"
              ? "error"
              : quotaStatus === "pending"
                ? "disabled"
                : runtimeTone ?? operationalStatusTone(operationalStatus);
        const statusIndicatorLabel = quotaStatus === "updated" ? operationalLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${operationalLabel}`;
        const indicatorLabel = runtimeHint ? `${statusIndicatorLabel} · ${runtimeHint}` : statusIndicatorLabel;
        const selectedAccount = selected.includes(account.id);
        return <Fragment key={account.id}>
        {groupByPlan && plan.id !== previousPlan ? <div className="account-plan-group-heading" role="presentation"><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /><span>{t("accounts.groupCount", { count: visiblePlanCounts.get(plan.id) ?? 0 })}</span></div> : null}
        <article className={`account-card${selectedAccount ? " selected" : ""}`} role="listitem">
          <div className="account-card-main">
            {displayedErrorCode
              ? <IconButton className="account-kind-icon account-status-button" data-status="error" label={accountErrorLabel(displayedErrorCode, t)} icon={<UserRound aria-hidden />} onClick={() => setErrorDetails(account)} />
              : <StatusIcon className="account-kind-icon" status={indicatorTone} label={indicatorLabel}><UserRound aria-hidden /></StatusIcon>}
            <div className="account-identity">
              <strong className={accountIdentitiesVisible ? "revealed" : undefined} title={account.label}>{account.label}</strong>
              <div className="account-identity-meta"><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></div>
            </div>
            <div className="account-card-header-actions">
              <ActionMenu className="account-row-menu">
                {errorCode ? <ActionMenuItem icon={<CircleAlert aria-hidden />} onClick={() => setErrorDetails(account)}>{t("accounts.errorDetailsTitle")}</ActionMenuItem> : null}
                {mode === "local" && requiresAccountReauthentication(account) ? <ActionMenuItem icon={<LogIn aria-hidden />} onClick={() => onReauthenticate(account)}>{t("accounts.reauthenticate")}</ActionMenuItem> : null}
                {onServer ? <ActionMenuItem icon={<Download aria-hidden />} disabled={Boolean(busy)} onClick={() => void returnToComputer(account)}>{t("accounts.returnToComputer")}</ActionMenuItem> : null}
                {onServer ? <ActionMenuItem danger icon={<Power aria-hidden />} disabled={Boolean(busy)} onClick={() => void recoverLocally(account)}>{t("accounts.forceActivateLocal")}</ActionMenuItem> : null}
                <ActionMenuItem icon={<Download aria-hidden />} disabled={!canExport || !account.secretAvailable} onClick={() => onExport([account.id])}>{t("accounts.exportOne", { name: account.label })}</ActionMenuItem>
                {!onServer ? <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void perform(`enable-${account.id}`, () => mode === "local" ? relayCommands.setAccountEnabled(account.id, !account.enabled) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled: !account.enabled }), "feedback.saved"); }}>{account.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem> : null}
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t(onServer ? "accounts.deleteLocalRecoveryConfirm" : mode === "remote" ? "accounts.deleteRemoteConfirm" : "accounts.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${account.id}`, () => mode === "local" ? relayCommands.deleteAccount(account.id) : relayCommands.remoteAction({ type: "delete_account", id: account.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
              <IconButton className="account-select-button" label={selectedAccount ? t("accounts.deselect", { name: account.label }) : t("accounts.select", { name: account.label })} icon={selectedAccount ? <Check aria-hidden /> : <Square aria-hidden />} aria-pressed={selectedAccount} onClick={() => toggleSelected(account.id)} />
            </div>
          </div>
          <div className="account-card-quota compact-quota-layout">
            {accountHasQuotaWindows(account) ? <QuotaStack snapshot={account.quota} nowMs={nowMs} concise /> : <AccountQuotaRefreshState account={account} />}
            {mode === "local" ? <ResetCreditsControl account={account} onCompleted={() => refresh()} /> : null}
          </div>
          <div className={`account-subscription-line${subscriptionEnded ? " expired" : ""}`} title={[subscriptionEnd.date, subscriptionEnd.relative].filter(Boolean).join(" · ")}><CalendarDays aria-hidden /><span>{subscriptionEnd.date}</span>{subscriptionEnd.relative ? <><span className="account-subscription-separator" aria-hidden>·</span><span className="account-subscription-countdown">{subscriptionEnd.relative}</span></> : null}</div>
          {runtimeHint ? <div className="account-runtime-line" data-warning={modelRetries.length > 0} title={runtimeHint}><Clock3 aria-hidden /><span>{runtimeHint}</span></div> : null}
          {accountValueVisible ? <AccountValueStrip account={account} /> : null}
          <footer className="account-card-footer"><div className="account-card-actions">
            {onServer
              ? <IconButton label={t("accounts.onServerHint")} icon={<Server aria-hidden />} disabled />
              : <IconButton className={participates ? "danger" : ""} label={poolActionLabel} icon={participates ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `pool-${account.id}`} onClick={() => void perform(`pool-${account.id}`, () => updateParticipation(account, !participates), "feedback.saved")} />}
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
  const status = accountQuotaRefreshState(account);
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
  return Boolean(account.quota.primary || account.quota.secondary || account.quota.supplemental?.some((item) => !isFastSupplementalQuota(item)));
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
