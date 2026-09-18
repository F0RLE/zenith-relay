import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, ArrowRight, CheckCheck, CircleAlert, CircleCheck, CirclePause, Clock3, Cloud, Coins, Cpu, DollarSign, Gauge, ListMinus, Loader2, Pencil, RefreshCw, UserRound, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, DefaultServiceTier } from "../../api/types";
import { accountQuotaRefreshState, currentAccountErrorCode, operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import {
  refreshAllAccountQuotas,
  refreshOneAccountQuota,
  type AccountQuotaRefreshReport,
} from "../../accountQuotaRefresh";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { ResetCreditsControl } from "../../components/ResetCreditsControl";
import { AccountPlanBadge, Button, EmptyState, IconButton, StatusIcon, accountErrorLabel, useConfirm } from "../../components/Ui";
import { AccountValueStrip } from "../../components/AccountValueStrip";
import { AccountProviderQuotaStrip } from "../../components/AccountProviderQuotaStrip";
import { AccountQuotaPanel } from "../../components/AccountQuotaPanel";
import { AccountSubscriptionLine } from "../../components/AccountSubscriptionLine";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { formatNumber } from "../../numberFormatting";
import { activeRequestCount, upcomingModelRetries } from "../../routingOrder";
import { memberName, type PoolMember } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { SourceStatsPanel } from "../../components/SourceStatsPanel";
import { settledSourceStats, type SourceStatsState } from "../../sourceStatsModel";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayActivity, useRelayState } from "../../state/relayStateContext";
import { AccountErrorDialog } from "../connections/AccountsTable";
import { PoolSpeedControl } from "./PoolSpeedControl";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
  poolMemberSourceIds,
  poolMemberStatusCounts,
  poolProviderCreditsSummary,
  poolRoutingAvailability,
} from "./poolMembersModel";

type Member = PoolMember;
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
  const sourceStatsRequests = useRef<Record<string, number>>({});
  const poolMembers: Member[] = useMemo(
    () => runtime ? poolMembersFromRuntime(runtime) : EMPTY_POOL_MEMBERS,
    [runtime?.accounts, runtime?.sources],
  );
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const runtimeByMember = useMemo(
    () => poolMemberRuntimeStates(poolMembers, runtimeOrder, runtimeActivity),
    [poolMembers, runtimeActivity, runtimeOrder],
  );
  const members = useMemo(() => orderedPoolMembers(poolMembers, runtimeOrder), [poolMembers, runtimeOrder]);
  const providerCredits = useMemo(() => poolProviderCreditsSummary(members), [members]);
  const providerCreditsValue = providerCredits == null
    ? null
    : providerCredits.kind === "unlimited"
      ? "∞"
      : formatNumber(providerCredits.availableCredits, i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 1 });
  const visibleModelIds = runtime?.gateway.visibleModelIds ?? EMPTY_VISIBLE_MODELS;
  const sourceIds = useMemo(() => poolMemberSourceIds(members), [members]);
  const sourceStatsScope = JSON.stringify(members.filter((member) => member.kind === "source").map((source) => [source.id, source.baseUrl, source.secretAvailable]).sort());
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
  const refreshSourceStats = useCallback(async (sourceId: string, refreshModels = false, operationManaged = false) => {
    if (mode === "zenith") return;
    const generation = sourceStatsGeneration.current;
    const request = (sourceStatsRequests.current[sourceId] ?? 0) + 1;
    sourceStatsRequests.current[sourceId] = request;
    const isCurrent = () => generation === sourceStatsGeneration.current && sourceStatsRequests.current[sourceId] === request;
    setSourceStats((current) => ({
      ...current,
      [sourceId]: { value: current[sourceId]?.value ?? null, loading: true, failed: false },
    }));
    let modelRefreshError: unknown;
    if (refreshModels && mode === "local") {
      const refreshModels = () => relayCommands.refreshSourceData(sourceId);
      if (operationManaged) {
        try { await refreshModels(); } catch (error) { modelRefreshError = error; }
      } else await perform(`source-data-refresh-${sourceId}`, refreshModels, "feedback.refreshed");
    }
    try {
      const value = await (mode === "local" ? relayCommands.localSourceStats(sourceId) : relayCommands.remoteSourceStats(sourceId));
      if (!isCurrent()) return;
      setSourceStats((current) => ({ ...current, [sourceId]: settledSourceStats(current[sourceId]?.value ?? null, value) }));
    } catch {
      if (!isCurrent()) return;
      setSourceStats((current) => ({
        ...current,
        [sourceId]: { value: current[sourceId]?.value ?? null, loading: false, failed: true },
      }));
    }
    if (modelRefreshError) throw modelRefreshError;
  }, [mode]);
  useEffect(() => {
    sourceStatsGeneration.current += 1;
    sourceStatsRequests.current = {};
    setSourceStats({});
    if (mode === "zenith" || !sourceIds) return;
    for (const sourceId of sourceIds.split("\n")) void refreshSourceStats(sourceId);
    return () => { sourceStatsGeneration.current += 1; };
  }, [mode, refreshSourceStats, sourceIds, sourceStatsScope]);
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
  const availability = poolRoutingAvailability(members, visibleModelIds, activeRequestTotal);
  const noAvailableModels = availability === "noModels";
  const counts = poolMemberStatusCounts(members);
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
          : t(availability === "ready" ? "pool.awaitingRoute" : "pool.priorityEmpty");
  const routingSummary = firstActiveMember
    ? activeMembers.length === 1
      ? `${t("pool.currentRoute")}: ${memberName(firstActiveMember)}`
      : activeMembers.length > 1
        ? t("pool.activeRoutes", { count: activeMembers.length })
        : idleRouteSummary
    : activeMembers.length > 1
      ? t("pool.activeRoutes", { count: activeMembers.length })
      : idleRouteSummary;
  const nextRouteSummary = firstActiveMember && nextMember
    ? `${t("pool.nextRoute")}: ${memberName(nextMember)}`
    : null;
  const unavailableMembers = members.filter((member) => member.operationalStatus === "unavailable");
  const unavailableRouteErrors = [...new Set(unavailableMembers
    .map((member) => member.kind === "source" ? member.lastErrorCode?.trim() : currentAccountErrorCode(member))
    .filter((code): code is string => Boolean(code))
    .map((code) => accountErrorLabel(code, t)))];
  const routingAlert = noAvailableModels
    ? <div className="pool-routing-alert" role="alert"><CircleAlert aria-hidden /><span><strong>{t("pool.noAvailableModels")}</strong><small>{t("pool.noAvailableModelsHint")}</small></span></div>
    : availability === "unavailable"
      ? <div className="pool-routing-alert" role="alert"><CircleAlert aria-hidden /><span><strong>{t("pool.noAvailableRoute")}</strong><small>{t("pool.noAvailableRouteHint", { quotaWait: counts.quotaWait, unavailable: unavailableMembers.length, disabled: counts.disabled })}{unavailableRouteErrors.length ? ` ${t("pool.routeErrors", { errors: unavailableRouteErrors.slice(0, 3).join("; ") })}` : ""}</small></span></div>
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
  const updateServiceTier = async (defaultServiceTier: DefaultServiceTier) => {
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
  return <>
    <div className="pool-controls" role="group" aria-label={t("pool.priorityTitle")}>
      <div className="table-toolbar pool-member-toolbar">
        <div className="pool-priority-label" data-relay-tooltip={t("pool.priorityHint")}><Activity aria-hidden /><h2>{t("pool.priorityTitle")}</h2></div>
        <div className="inline-actions pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <PoolSpeedControl
              key={mode}
              value={serviceTier}
              disabled={!supportsRoutingSettings}
              saving={pendingServiceTier !== null || busy === "pool-service-tier"}
              onChange={(value) => void updateServiceTier(value)}
            />
            <IconButton label={t("pool.routingSettings")} icon={<Gauge aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={onRoutingPolicy} />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            {hasAccountMembers ? <IconButton className="account-calculation-toggle" label={t(accountValueVisible ? "pool.hideCalculation" : "pool.showCalculation")} icon={<DollarSign aria-hidden />} aria-pressed={accountValueVisible} onClick={() => setAccountValueVisible(!accountValueVisible)} /> : null}
            <IconButton label={t("pool.refreshQuotas")} icon={busy === "pool-quota-refresh" ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} aria-busy={busy === "pool-quota-refresh"} disabled={busy === "pool-quota-refresh" || !canRefreshQuota || !refreshableMemberCount} title={!refreshableMemberCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()} />
          </div>
        </div>
      </div>
      <div className="pool-runtime-strip">
        <div className="pool-route-summary">
          <strong className="pool-current-route" data-active={activeRequestTotal > 0}>{routingSummary}</strong>
          {nextRouteSummary ? <span className="pool-next-route"><ArrowRight aria-hidden /><span>{nextRouteSummary}</span></span> : null}
        </div>
        {activeRequestSummary ? <span className="pool-active-models" data-active-request-count={activeRequestTotal} data-active-models={activeModels.map(({ model, requestCount }) => `${model}:${requestCount}`).join(",")}><Cpu aria-hidden /><span>{activeRequestSummary}</span></span> : null}
      </div>
      <div className="pool-summary connection-status-summary" data-has-provider-credits={providerCreditsValue != null ? "true" : "false"}>
        <div data-tone={counts.rotation ? "ready" : "muted"}><CircleCheck aria-hidden /><strong>{counts.rotation}</strong><span>{t("pool.memberStatus.rotation")}</span></div>
        <div data-tone={counts.quotaWait ? "warning" : "muted"}><Clock3 aria-hidden /><strong>{counts.quotaWait}</strong><span>{t("pool.memberStatus.quotaWait")}</span></div>
        <div data-tone={counts.errors ? "error" : "muted"}><CircleAlert aria-hidden /><strong>{counts.errors}</strong><span>{t("accounts.summary.errors")}</span></div>
        <div data-tone="muted"><CirclePause aria-hidden /><strong>{counts.disabled}</strong><span>{t("pool.memberStatus.disabled")}</span></div>
        {providerCreditsValue != null ? <div className="pool-summary-provider-credits" data-summary="provider-credits" data-relay-tooltip={t("pool.totalProviderCreditsHint")}><Coins aria-hidden /><strong>{providerCreditsValue}</strong><span>{t("pool.totalProviderCredits")}</span></div> : null}
      </div>
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
        const isCurrent = activeRequestCount(runtimeState) > 0;
        const isNext = !isCurrent && nextMember?.kind === member.kind && nextMember.id === member.id;
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
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}${isCurrent ? " current" : ""}${isNext ? " next" : ""}${isLastUsed ? " last-used" : ""}`} role="listitem" data-member-label={member.kind === "source" ? member.name : member.label} data-current={isCurrent ? "true" : "false"} data-next={isNext ? "true" : "false"} data-last-used={isLastUsed ? "true" : "false"} data-member-kind={member.kind}>
          <header className="pool-member-card-header">
            {member.kind === "account" && displayedErrorCode
              ? <IconButton className="pool-member-kind-icon" data-status="error" label={indicatorLabel} icon={<UserRound aria-hidden />} onClick={() => setErrorDetails(member)} />
              : <StatusIcon className="pool-member-kind-icon" status={indicatorTone} label={[indicatorHint, codexInterface ? t("pool.codexInterfaceHint") : null].filter(Boolean).join(" · ")} showTooltip={!(member.kind === "source" && visibleMemberErrorCode)}>{member.kind === "source" ? <Cloud aria-hidden /> : <UserRound aria-hidden />}</StatusIcon>}
            <div className="pool-member-identity">
              <strong className="pool-member-name" data-relay-tooltip={member.kind === "account" && identity !== detail ? `${identity} · ${detail}` : identity}>{identity}</strong>
              <div className="pool-member-meta">{member.kind === "account" ? <AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} /> : <small data-relay-tooltip={detail}>{detail}</small>}</div>
            </div>
          </header>
          <div className={`pool-member-card-quota${member.kind === "account" ? " compact-quota-layout" : ""}`}>
            {member.kind === "account" ? <AccountQuotaPanel account={member} nowMs={nowMs} onReauthenticate={onReauthenticate} /> : <SourceStatsPanel source={member} {...(sourceStats[member.id] ? { state: sourceStats[member.id] } : {})} />}
            {mode === "local" && member.kind === "account" ? <ResetCreditsControl account={member} onCompleted={() => refresh()} /> : null}
          </div>
          {member.kind === "account" ? <AccountProviderQuotaStrip account={member} /> : null}
          {member.kind === "account" ? <><AccountSubscriptionLine activeUntilMs={member.subscription.activeUntilMs} nowMs={nowMs} />{runtimeHint ? <div className="account-runtime-line" data-warning={modelRetries.length > 0}><Clock3 aria-hidden /><span>{runtimeHint}</span></div> : null}</> : <div className="pool-member-context" data-kind="source"><div className="pool-member-runtime-meta"><div><span>{t("pool.operationMode")}</span><strong>{t(`pool.rotationModes.${runtime?.gateway.poolRouting?.mode ?? "smart"}`)}</strong></div><div><span>{t("pool.parallelism")}</span><strong>{parallelRequests}</strong></div></div></div>}
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
