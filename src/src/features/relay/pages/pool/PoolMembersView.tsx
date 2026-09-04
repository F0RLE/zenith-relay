import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, CheckCheck, CircleAlert, Clock3, Cloud, DollarSign, Gauge, ListMinus, Loader2, LogIn, Pencil, RefreshCw, UserRound, X, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, DefaultServiceTier, SourceStats, SourceSummary } from "../../api/types";
import { accountQuotaRefreshState, currentAccountErrorCode, operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import {
  refreshAllAccountQuotas,
  refreshOneAccountQuota,
  type AccountQuotaRefreshReport,
} from "../../accountQuotaRefresh";
import { subscriptionExpiryFormatter, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { ResetCreditsControl } from "../../components/ResetCreditsControl";
import { AccountPlanBadge, Button, EmptyState, IconButton, QuotaStack, StatusIcon, accountErrorLabel, useConfirm } from "../../components/Ui";
import { AccountValueStrip } from "../../components/AccountValueStrip";
import { formatDetailedRemainingTime, isFastSupplementalQuota } from "../../quotaFormatting";
import { activeRequestCount, apiSourceRole, upcomingModelRetries } from "../../routingOrder";
import { memberName, type PoolMember } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { formatApiEquivalent, formatProviderMicroUsd } from "../../poolFormatting";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayActivity, useRelayState } from "../../state/relayStateContext";
import { formatFullNumber } from "../../usageTotals";
import { AccountErrorDialog } from "../connections/AccountsTable";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
  poolMemberSourceIds,
  poolMemberStatusCounts,
  memberCanRoute,
} from "./poolMembersModel";

type Member = PoolMember;
type SourceStatsState = { value: SourceStats | null; loading: boolean; failed: boolean };
const EMPTY_POOL_MEMBERS: Member[] = [];
const EMPTY_RUNTIME_ORDER: CandidateRuntimeSnapshot[] = [];
const EMPTY_VISIBLE_MODELS: string[] = [];

export function PoolMembersView({ onAdd, onRoutingPolicy, onReauthenticate, supportsRoutingSettings }: { onAdd: () => void; onRoutingPolicy: () => void; onReauthenticate: (account: AccountSummary) => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, refresh, busy, codexPoolOauthSelection, accountValueVisible, setAccountValueVisible } = useRelayState();
  const runtimeActivity = useRelayActivity();
  const confirm = useConfirm();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const [pendingServiceTier, setPendingServiceTier] = useState<DefaultServiceTier | null>(null);
  const serviceTier = pendingServiceTier ?? runtime?.gateway.defaultServiceTier ?? "standard";
  const routingStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const [sourceStats, setSourceStats] = useState<Record<string, SourceStatsState>>({});
  const sourceStatsGeneration = useRef(0);
  const poolMembers: Member[] = useMemo(
    () => runtime ? poolMembersFromRuntime(runtime) : EMPTY_POOL_MEMBERS,
    [runtime?.accounts, runtime?.sources],
  );
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const runtimeByMember = useMemo(() => poolMemberRuntimeStates(poolMembers, runtimeOrder), [poolMembers, runtimeOrder]);
  const members = useMemo(() => orderedPoolMembers(poolMembers, runtimeOrder), [poolMembers, runtimeOrder]);
  const visibleModelIds = runtime?.gateway.visibleModelIds ?? EMPTY_VISIBLE_MODELS;
  const sourceIds = useMemo(() => poolMemberSourceIds(members), [members]);
  const memberTimestamps = useMemo(() => members.flatMap((member) => [
    ...(member.kind === "account" ? [
      member.subscription.activeUntilMs,
      member.quota.primary?.resetAtMs,
      member.quota.secondary?.resetAtMs,
      ...(member.quota.supplemental ?? []).map((item) => item.window.resetAtMs),
    ] : []),
    runtimeByMember.get(member.id)?.nextRetryAtMs,
  ]), [members, runtimeByMember]);
  const nowMs = useRelativeTimeClock(memberTimestamps);
  const subscriptionExpiryFormat = subscriptionExpiryFormatter(i18n.language);
  const refreshSourceStats = useCallback(async (sourceId: string, refreshModels = false, operationManaged = false) => {
    if (mode === "zenith") return;
    const generation = sourceStatsGeneration.current;
    setSourceStats((current) => ({
      ...current,
      [sourceId]: { value: current[sourceId]?.value ?? null, loading: true, failed: false },
    }));
    try {
      if (refreshModels && mode === "local") {
        const refresh = () => relayCommands.refreshSourceData(sourceId);
        if (operationManaged) await refresh();
        else await perform(`source-data-refresh-${sourceId}`, refresh, "feedback.refreshed");
      }
      const value = await (mode === "local" ? relayCommands.localSourceStats(sourceId) : relayCommands.remoteSourceStats(sourceId));
      if (generation !== sourceStatsGeneration.current) return;
      setSourceStats((current) => ({ ...current, [sourceId]: { value, loading: false, failed: false } }));
    } catch {
      if (generation !== sourceStatsGeneration.current) return;
      setSourceStats((current) => ({
        ...current,
        [sourceId]: { value: current[sourceId]?.value ?? null, loading: false, failed: true },
      }));
    }
  }, [mode]);
  useEffect(() => {
    sourceStatsGeneration.current += 1;
    setSourceStats({});
    if (mode === "zenith" || !sourceIds) return;
    for (const sourceId of sourceIds.split("\n")) void refreshSourceStats(sourceId);
    return () => { sourceStatsGeneration.current += 1; };
  }, [mode, refreshSourceStats, sourceIds]);
  const {
    activeMembers,
    nextMember,
    activeRequestTotal,
    activeModels,
    lastUsedRuntime,
    lastUsedMember,
    lastActivityMember,
  } = useMemo(() => poolActivityState(members, runtimeByMember, runtimeOrder, runtimeActivity, visibleModelIds), [members, runtimeActivity, runtimeByMember, runtimeOrder, visibleModelIds]);
  const activeModelList = activeModels
    .map(({ model, requestCount }) => requestCount > 1 ? t("pool.activeModelCount", { model, count: requestCount }) : model)
    .join(" · ");
  const activeRequestSummary = activeRequestTotal > 0
    ? activeModelList
      ? t("pool.activeRequests", { count: activeRequestTotal, models: activeModelList })
      : t("pool.activeRequestsUnknown", { count: activeRequestTotal })
    : null;
  const hasRoutableModel = members.some((member) => memberCanRoute(member, visibleModelIds));
  const noAvailableModels = visibleModelIds.length === 0 || !hasRoutableModel;
  const hasAvailableRoute = nextMember != null;
  const firstActiveMember = activeMembers[0];
  const recentRoute = !activeMembers.length && lastActivityMember && nextMember && lastActivityMember.id !== nextMember.id
    ? `${t("pool.lastRoute")}: ${memberName(lastActivityMember)} · ${t("pool.nextRoute")}: ${memberName(nextMember)}`
    : null;
  const idleRouteSummary = noAvailableModels
    ? t("pool.noAvailableModels")
    : recentRoute
      ? recentRoute
      : nextMember
        ? `${t("pool.nextRoute")}: ${memberName(nextMember)}`
        : (lastActivityMember ?? lastUsedMember)
          ? `${t("pool.lastRoute")}: ${memberName(lastActivityMember ?? lastUsedMember!)}`
          : t(hasAvailableRoute ? "pool.awaitingRoute" : "pool.priorityEmpty");
  const routingSummary = firstActiveMember
    ? activeMembers.length === 1
      ? `${t("pool.currentRoute")}: ${memberName(firstActiveMember)}`
      : activeMembers.length > 1
        ? t("pool.activeRoutes", { count: activeMembers.length })
        : idleRouteSummary
    : activeMembers.length > 1
      ? t("pool.activeRoutes", { count: activeMembers.length })
      : idleRouteSummary;
  const unavailableRouteErrors = members
    .map((member) => member.kind === "source" ? member.lastErrorCode?.trim() : currentAccountErrorCode(member))
    .filter((code): code is string => Boolean(code));
  const routingAlert = noAvailableModels
    ? <div className="pool-routing-alert" role="alert"><CircleAlert aria-hidden /><span><strong>{t("pool.noAvailableModels")}</strong><small>{t("pool.noAvailableModelsHint")}{visibleModelIds.length > 0 ? ` ${t("pool.noEligibleSourceError")}` : ""}{unavailableRouteErrors.length ? ` ${t("pool.routeErrors", { errors: unavailableRouteErrors.slice(0, 3).join(", ") })}` : ""}</small></span></div>
    : !activeMembers.length && !nextMember && unavailableRouteErrors.length
      ? <div className="pool-routing-alert" role="alert"><CircleAlert aria-hidden /><span><strong>{t("pool.noAvailableRoute")}</strong><small>{t("pool.noAvailableRouteHint", { errors: unavailableRouteErrors.slice(0, 3).join(", ") })}</small></span></div>
      : null;
  const selected = members.find((member) => `${member.kind}:${member.id}` === selectedId) ?? null;
  const remove = async (member: Member) => {
    const ok = await perform(`pool-remove-${member.id}`, () => updatePoolMembership(mode, {
      accountIds: member.kind === "account" ? [member.id] : [],
      sourceIds: member.kind === "source" ? [member.id] : [],
      inPool: false,
    }), "feedback.saved");
    if (ok) setSelectedId(null);
  };
  const confirmRemove = async (member: Member) => {
    const name = member.kind === "source" ? member.name : member.label;
    if (!await confirm(t("pool.removeMemberConfirm", { name }), { danger: true, confirmLabel: t("pool.removeMember") })) return;
    await remove(member);
  };
  const quotaAccountCount = members.filter((member) => member.kind === "account" && member.enabled).length;
  const refreshableSourceIds = members
    .filter((member): member is Extract<Member, { kind: "source" }> => member.kind === "source" && member.secretAvailable)
    .map((member) => member.id);
  const refreshableMemberCount = quotaAccountCount + refreshableSourceIds.length;
  const hasAccountMembers = members.some((member) => member.kind === "account");
  const refreshQuotas = async () => {
    let report: AccountQuotaRefreshReport | null = null;
    const ok = await perform("pool-quota-refresh", async () => {
      if (quotaAccountCount) report = await refreshAllAccountQuotas(mode);
      await Promise.all(refreshableSourceIds.map((sourceId) => refreshSourceStats(sourceId, mode === "local", true)));
    });
    if (ok && report) setQuotaReport(report);
  };
  const refreshAccountQuota = (account: AccountSummary) => perform(
    `pool-account-quota-${account.id}`,
    () => refreshOneAccountQuota(mode, account.id),
    "feedback.refreshed",
  );
  const updateServiceTier = async (fast: boolean) => {
    const defaultServiceTier = fast ? "fast" : "standard";
    if (defaultServiceTier === serviceTier) return;
    setPendingServiceTier(defaultServiceTier);
    try {
      await perform("pool-service-tier", () => persistRoutingPolicy(mode, {
        maxRetryCandidates: runtime?.gateway.maxRetryCandidates ?? 3,
        cooldownAfterFailures: runtime?.gateway.cooldownAfterFailures ?? 3,
        keepLastCandidateAvailable: runtime?.gateway.keepLastCandidateAvailable ?? true,
        routingStrategy,
        defaultServiceTier,
        subscriptionPlanOrder: runtime?.gateway.subscriptionPlanOrder ?? [],
      }));
    } finally {
      setPendingServiceTier(null);
    }
  };
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={onAdd}>{t("pool.addMember")}</Button>} />;
  const counts = poolMemberStatusCounts(members);
  return <>
    <div className="pool-controls">
      <div className="table-toolbar pool-member-toolbar">
        <div className="pool-priority-label" title={t("pool.priorityHint")}><Activity aria-hidden /><span><strong>{t("pool.priorityTitle")}</strong><small>{routingSummary}</small>{activeRequestSummary ? <small className="pool-active-models" data-active-request-count={activeRequestTotal} data-active-models={activeModels.map(({ model, requestCount }) => `${model}:${requestCount}`).join(",")} title={activeRequestSummary}>{activeRequestSummary}</small> : null}</span></div>
        <div className="inline-actions pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <label className="pool-speed-control" data-fast={serviceTier === "fast" ? "true" : "false"} title={t("pool.serviceTierHint")}>
              <Zap aria-hidden />
              <span className="pool-speed-copy"><small>{t("pool.serviceTier")}</small><strong>{t(`pool.serviceTiers.${serviceTier}`)}</strong></span>
              <input type="checkbox" role="switch" aria-label={t("pool.serviceTier")} checked={serviceTier === "fast"} disabled={busy === "pool-service-tier"} onChange={(event) => void updateServiceTier(event.target.checked)} />
              <span className="pool-speed-track" aria-hidden><span /></span>
            </label>
            <IconButton label={t("pool.routingSettings")} icon={<Gauge aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={onRoutingPolicy} />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            {hasAccountMembers ? <IconButton className="account-calculation-toggle" label={t(accountValueVisible ? "pool.hideCalculation" : "pool.showCalculation")} icon={<DollarSign aria-hidden />} aria-pressed={accountValueVisible} onClick={() => setAccountValueVisible(!accountValueVisible)} /> : null}
            <Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "pool-quota-refresh"} disabled={!canRefreshQuota || !refreshableMemberCount} title={!refreshableMemberCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()}>{t("pool.refreshQuotas")}</Button>
          </div>
        </div>
      </div>
      <div className="pool-summary"><div><span>{t("pool.memberStatus.rotation")}</span><i aria-hidden="true">—</i><strong>{counts.rotation}</strong></div><div><span>{t("pool.memberStatus.quotaWait")}</span><i aria-hidden="true">—</i><strong>{counts.quotaWait}</strong></div><div><span>{t("accounts.summary.errors")}</span><i aria-hidden="true">—</i><strong>{counts.errors}</strong></div><div><span>{t("pool.memberStatus.disabled")}</span><i aria-hidden="true">—</i><strong>{counts.disabled}</strong></div></div>
    </div>
    {routingAlert}
    {quotaReport ? <div className={`account-quota-report${quotaReport.failed ? " has-errors" : ""}`} role="status"><CheckCheck aria-hidden /><span>{t("accounts.quotaRefreshReport", quotaReport)}</span><button type="button" aria-label={t("common.close")} onClick={() => setQuotaReport(null)}><X aria-hidden /></button></div> : null}
    <div className="pool-member-list" role="list" aria-label={t("pool.members")}>
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const runtimeState = runtimeByMember.get(member.id);
        const statusKey = member.operationalStatus;
        const statusTone = operationalStatusTone(statusKey);
        const quotaStatus = member.kind === "account" ? accountQuotaRefreshState(member) : "updated";
        const errorCode = member.kind === "account" ? currentAccountErrorCode(member) : null;
        const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
        const codexInterface = member.kind === "account" && codexPoolOauthSelection === member.id;
        const identity = member.kind === "source" ? member.name : member.identityHint || member.label;
        const detail = member.kind === "source"
          ? `${member.wireApi} · ${member.baseUrl}`
          : member.label;
        const subscriptionExpiry = member.kind === "account"
          ? member.subscription.activeUntilMs == null
            ? { date: t("pool.subscriptionExpiryUnknown"), remaining: null }
            : { date: subscriptionExpiryFormat.format(member.subscription.activeUntilMs), remaining: formatDetailedRemainingTime(member.subscription.activeUntilMs, nowMs, t) }
          : null;
        const isCurrent = activeRequestCount(runtimeState) > 0;
        const isLastUsed = !isCurrent && runtimeState != null && runtimeState.lastUsedAtMs != null && runtimeState.lastUsedAtMs === lastUsedRuntime?.lastUsedAtMs;
        const modelRetries = upcomingModelRetries(runtimeState, nowMs);
        const firstModelRetry = modelRetries[0];
        const modelRetryHint = firstModelRetry
          ? t("pool.modelRetryAt", {
            models: modelRetries.map((retry) => retry.model).join(", "),
            time: formatDetailedRemainingTime(firstModelRetry.retryAtMs, nowMs, t),
          })
          : null;
        const memberErrorCode = member.kind === "source" ? member.lastErrorCode?.trim() : errorCode;
        const visibleMemberErrorCode = member.kind === "source" ? memberErrorCode : displayedErrorCode;
        const runtimeTone = statusKey === "rotation"
          ? member.kind === "source"
            ? transientCandidateTone(runtimeState, nowMs, true)
            : modelRetries.length > 0
              ? "warning"
              : transientCandidateTone(runtimeState, nowMs, false)
          : null;
        const indicatorTone = visibleMemberErrorCode
          ? "error"
          : statusKey === "unavailable" || statusKey === "disabled"
          ? statusTone
          : quotaStatus === "refreshing"
            ? "disabled"
            : quotaStatus === "failed" || quotaStatus === "requires_reauth"
              ? "error"
              : quotaStatus === "pending"
                ? "disabled"
                : runtimeTone ?? statusTone;
        const runtimeHint = runtimeState?.halfOpen
          ? t("pool.recoveryProbe")
          : modelRetryHint
            ? modelRetryHint
            : member.kind === "source" && runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
            ? t("pool.retryAt", { time: formatDetailedRemainingTime(runtimeState.nextRetryAtMs, nowMs, t) })
            : undefined;
        const parallelRequests = activeRequestCount(runtimeState);
        // Source failures are represented by the status icon. Keep only a
        // retry countdown in the card tooltip so the same error is not shown
        // again by the WebView title and the card footer.
        const sourceRuntimeTitle = runtimeHint || undefined;
        const editLabel = `${t("pool.editMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removeLabel = `${t("pool.removeMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removing = busy === `pool-remove-${member.id}`;
        const statusLabel = t(`pool.memberStatus.${statusKey}`);
        const indicatorLabel = visibleMemberErrorCode
          ? member.kind === "account" ? accountErrorLabel(visibleMemberErrorCode, t) : t("pool.runtimeError", { code: visibleMemberErrorCode })
          : quotaStatus === "updated" ? statusLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${statusLabel}`;
        const indicatorHint = member.kind === "source"
          ? [indicatorLabel, runtimeHint].filter(Boolean).join(" · ")
          : [runtimeHint, indicatorLabel].filter(Boolean).join(" · ");
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}${isCurrent ? " current" : ""}${isLastUsed ? " last-used" : ""}`} role="listitem" title={[codexInterface ? t("pool.codexInterfaceHint") : null, member.kind === "source" ? sourceRuntimeTitle : null].filter(Boolean).join(" · ") || undefined} data-member-label={member.kind === "source" ? member.name : member.label} data-current={isCurrent ? "true" : "false"} data-last-used={isLastUsed ? "true" : "false"} data-member-kind={member.kind}>
          <header className="pool-member-card-header">
            {member.kind === "account" && displayedErrorCode
              ? <IconButton className="pool-member-kind-icon" data-status="error" label={indicatorLabel} icon={<UserRound aria-hidden />} onClick={() => setErrorDetails(member)} />
              : <StatusIcon className="pool-member-kind-icon" status={indicatorTone} label={indicatorHint} showTooltip={!(member.kind === "source" && visibleMemberErrorCode)}>{member.kind === "source" ? <Cloud aria-hidden /> : <UserRound aria-hidden />}</StatusIcon>}
            <div className="pool-member-identity">
              <strong className="pool-member-name" title={identity === detail ? identity : `${identity} · ${detail}`}>{identity}</strong>
              <div className="pool-member-meta">{member.kind === "account" ? <AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} /> : <small title={detail}>{detail}</small>}</div>
            </div>
          </header>
          <div className={`pool-member-card-quota${member.kind === "account" ? " compact-quota-layout" : ""}`}>
            {member.kind === "account" ? <PoolAccountQuota account={member} nowMs={nowMs} onReauthenticate={onReauthenticate} /> : <PoolSourceStats source={member} {...(sourceStats[member.id] ? { state: sourceStats[member.id] } : {})} />}
            {mode === "local" && member.kind === "account" ? <ResetCreditsControl account={member} onCompleted={() => refresh()} /> : null}
          </div>
          <div className="pool-member-context" data-kind={member.kind}>{member.kind === "account" ? <><span className="pool-member-subscription-date">{subscriptionExpiry?.date}</span>{subscriptionExpiry?.remaining ? <><span className="pool-member-context-separator" aria-hidden>·</span><span className="pool-member-subscription-expiry">{subscriptionExpiry.remaining}</span></> : null}{runtimeHint ? <><span className="pool-member-context-separator" aria-hidden>·</span><span className="pool-member-runtime-hint" data-warning="false">{runtimeHint}</span></> : null}</> : <div className="pool-member-runtime-meta"><div><span>{t("pool.operationMode")}</span><strong>{t(`sources.roles.${apiSourceRole(member.priority)}`)}</strong></div><div><span>{t("pool.parallelism")}</span><strong>{parallelRequests}</strong></div></div>}</div>
          {member.kind === "account" && accountValueVisible ? <AccountValueStrip account={member} /> : null}
          <footer className="pool-member-card-footer" data-kind={member.kind}>
            <div className="pool-member-actions">
              <IconButton className="danger" data-relay-context-action label={removeLabel} icon={removing ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />} disabled={removing} onClick={() => void confirmRemove(member)} onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                void remove(member);
              }} />
              {member.kind === "source" ? <IconButton label={t("pool.refreshSourceStats")} icon={sourceStats[member.id]?.loading ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={!member.secretAvailable || sourceStats[member.id]?.loading || Boolean(busy)} onClick={() => void refreshSourceStats(member.id, true)} /> : null}
              {member.kind === "account" ? <IconButton label={t("accounts.refreshQuota")} icon={busy === `pool-account-quota-${member.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={!canRefreshQuota || !member.secretAvailable || Boolean(busy)} onClick={() => void refreshAccountQuota(member)} /> : null}
              <IconButton label={editLabel} icon={<Pencil aria-hidden />} aria-haspopup="dialog" onClick={() => setSelectedId(memberId)} />
            </div>
          </footer>
        </article>;
      })}
    </div>
    {selected ? <PoolMemberEditor key={`${selected.kind}:${selected.id}`} member={selected} onClose={() => setSelectedId(null)} /> : null}
    {errorDetails ? <AccountErrorDialog account={errorDetails} onClose={() => setErrorDetails(null)} /> : null}
  </>;
}

function PoolAccountQuota({ account, nowMs, onReauthenticate }: { account: AccountSummary; nowMs: number; onReauthenticate: (account: AccountSummary) => void }) {
  const { t } = useTranslation();
  const hasQuota = Boolean(account.quota.primary || account.quota.secondary || account.quota.supplemental?.some((item) => !isFastSupplementalQuota(item)));
  // A signed-out account must ask for sign-in even when the last quota refresh
  // still reports a successful snapshot.
  const status = accountQuotaRefreshState(account);
  return <>{status === "requires_reauth" ? <button type="button" className={`account-quota-refresh-state ${status} is-action`} onClick={() => onReauthenticate(account)}><LogIn aria-hidden /><span>{t(`accounts.quotaRefreshStatus.${status}`)}</span></button> : !hasQuota ? <div className={`account-quota-refresh-state ${status}`} role="status">{status === "refreshing" ? <Loader2 className="spin" aria-hidden /> : status === "updated" ? <CheckCheck aria-hidden /> : status === "failed" ? <RefreshCw aria-hidden /> : <Clock3 aria-hidden />}<span>{t(`accounts.quotaRefreshStatus.${status}`)}</span></div> : <QuotaStack snapshot={account.quota} nowMs={nowMs} concise />}</>;
}

function PoolSourceStats({ source, state }: { source: SourceSummary; state?: SourceStatsState }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const stats = state?.value;
  const providerStats = stats != null && stats.provider !== "unsupported";
  const balance = state?.loading && !stats
    ? "…"
    : providerStats
      ? stats.balanceMicroUsd == null ? "—" : formatProviderMicroUsd(stats.balanceMicroUsd, locale)
      : state?.failed
        ? t("common.failed")
        : t("pool.sourceStatsUnsupported");
  const spent = providerStats && stats.spentMicroUsd != null
    ? formatProviderMicroUsd(stats.spentMicroUsd, locale)
    : formatApiEquivalent(source.apiEquivalent.microUsd, locale);
  const requests = providerStats
    ? stats.requests == null ? "—" : formatFullNumber(stats.requests, locale)
    : "—";
  return <dl className="pool-source-stats">
    <div title={state?.failed ? t("overview.sourceStatsUnavailable") : !providerStats && !state?.loading ? t("overview.sourceStatsUnsupported") : undefined}><dt>{t("overview.balance")}</dt><dd data-muted={!providerStats ? "true" : undefined}>{balance}</dd></div>
    <div title={!providerStats ? t("pool.apiEquivalentHint", { count: source.apiEquivalent.unpricedTokens }) : undefined}><dt>{providerStats ? t("overview.spent") : t("pool.apiEquivalent")}</dt><dd>{spent}</dd></div>
    <div><dt>{t("usage.requests")}</dt><dd>{requests}</dd></div>
    <div><dt>{t("common.models")}</dt><dd>{formatFullNumber(source.models.length, locale)}</dd></div>
  </dl>;
}
