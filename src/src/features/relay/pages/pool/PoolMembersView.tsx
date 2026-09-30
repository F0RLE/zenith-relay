import { useMemo, useState } from "react";
import { ArrowRight, CheckCheck, CircleAlert, CircleCheck, CirclePause, Clock3, Coins, Cpu, DollarSign, Loader2, RefreshCw, SlidersHorizontal, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, DefaultServiceTier } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import {
  refreshAllAccountQuotas,
  type AccountQuotaRefreshReport,
} from "../../accountQuotaRefresh";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { Button, EmptyState, IconButton, accountErrorLabel, useConfirm } from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { activeRequestCount } from "../../routingOrder";
import { memberName, type PoolMember } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useSourceStats } from "../../hooks/useSourceStats";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayActivity, useRelayState } from "../../state/relayStateContext";
import { AccountErrorDialog } from "../connections/AccountErrorDialog";
import { PoolSpeedControl } from "./PoolSpeedControl";
import { PoolMemberCard } from "./PoolMemberCard";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
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
  const { mode, runtime, perform, busy, accountValueVisible, setAccountValueVisible } = useRelayState();
  const runtimeActivity = useRelayActivity();
  const confirm = useConfirm();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const [pendingServiceTier, setPendingServiceTier] = useState<DefaultServiceTier | null>(null);
  const serviceTier = pendingServiceTier ?? runtime?.gateway.defaultServiceTier ?? "standard";
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
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
  const sourceMembers = members.filter((member): member is Extract<Member, { kind: "source" }> => member.kind === "source");
  const rotationMode = runtime?.gateway.poolRouting?.version === 2 ? runtime.gateway.poolRouting.mode : null;
  const { stats: sourceStats, refresh: readSourceStats } = useSourceStats(mode, sourceMembers);
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
  const refreshSourceStats = async (sourceId: string, refreshModels = false, operationManaged = false) => {
    if (mode === "zenith") return;
    let modelRefreshError: unknown;
    if (refreshModels && mode === "local") {
      const refreshModels = () => relayCommands.refreshSourceData(sourceId);
      if (operationManaged) {
        try { await refreshModels(); } catch (error) { modelRefreshError = error; }
      } else await perform(`source-data-refresh-${sourceId}`, refreshModels, "feedback.refreshed");
    }
    await readSourceStats(sourceId, refreshModels || operationManaged);
    if (modelRefreshError) throw modelRefreshError;
  };
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
  const routingAlert = noAvailableModels ? (
    <div className="pool-routing-alert" role="alert">
      <CircleAlert aria-hidden />
      <span>
        <strong>{t("pool.noAvailableModels")}</strong>
        <small>{t("pool.noAvailableModelsHint")}</small>
      </span>
    </div>
  ) : availability === "unavailable" ? (
    <div className="pool-routing-alert" role="alert">
      <CircleAlert aria-hidden />
      <span>
        <strong>{t("pool.noAvailableRoute")}</strong>
        <small>
          {t("pool.noAvailableRouteHint", {
            quotaWait: counts.quotaWait,
            unavailable: unavailableMembers.length,
            disabled: counts.disabled,
          })}
          {unavailableRouteErrors.length
            ? ` ${t("pool.routeErrors", { errors: unavailableRouteErrors.slice(0, 3).join("; ") })}`
            : ""}
        </small>
      </span>
    </div>
  ) : null;
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
  const updateServiceTier = async (defaultServiceTier: DefaultServiceTier) => {
    if (defaultServiceTier === serviceTier) return;
    setPendingServiceTier(defaultServiceTier);
    try {
      await perform("pool-service-tier", () => persistRoutingPolicy(mode, {
        maxRetryCandidates: runtime?.gateway.maxRetryCandidates ?? 3,
        defaultServiceTier,
      }));
    } finally {
      setPendingServiceTier(null);
    }
  };
  if (!members.length) {
    return (
      <EmptyState
        title={t("pool.emptyTitle")}
        description={t("pool.emptyDescription")}
        action={(
          <Button
            variant="primary"
            disabled={!canAdd}
            title={!canAdd ? t("remote.capabilityUnavailable") : undefined}
            onClick={onAdd}
          >
            {t("pool.addMember")}
          </Button>
        )}
      />
    );
  }
  return <>
    <div className="pool-controls workspace-controls" role="group" aria-label={t("pool.priorityTitle")}>
      <div className="pool-summary relay-status-summary" data-has-provider-credits={providerCreditsValue != null ? "true" : "false"}>
        <div data-tone={counts.rotation ? "ready" : "muted"}><CircleCheck aria-hidden /><strong>{counts.rotation}</strong><span>{t("pool.memberStatus.rotation")}</span></div>
        <div data-tone={counts.quotaWait ? "warning" : "muted"}><Clock3 aria-hidden /><strong>{counts.quotaWait}</strong><span>{t("pool.memberStatus.quotaWait")}</span></div>
        <div data-tone={counts.errors ? "error" : "muted"}><CircleAlert aria-hidden /><strong>{counts.errors}</strong><span>{t("accounts.summary.errors")}</span></div>
        <div data-tone="muted"><CirclePause aria-hidden /><strong>{counts.disabled}</strong><span>{t("pool.memberStatus.disabled")}</span></div>
        {providerCreditsValue != null ? (
          <div data-summary="provider-credits" data-relay-tooltip={t("pool.totalProviderCreditsHint")}>
            <Coins aria-hidden />
            <strong>{providerCreditsValue}</strong>
            <span>{t("pool.totalProviderCredits")}</span>
          </div>
        ) : null}
      </div>
      <div className="pool-member-toolbar">
        <div className="pool-priority-context">
          <div className="pool-priority-label" data-relay-tooltip={t("pool.priorityHint")}><h2>{t("pool.priorityTitle")}</h2></div>
          <div className="pool-runtime-strip">
            <div className="pool-route-summary">
              <strong className="pool-current-route" data-active={activeRequestTotal > 0}>{routingSummary}</strong>
              {nextRouteSummary ? <span className="pool-next-route"><ArrowRight aria-hidden /><span>{nextRouteSummary}</span></span> : null}
            </div>
            {activeRequestSummary ? (
              <span
                className="pool-active-models"
                data-active-request-count={activeRequestTotal}
                data-active-models={activeModels.map(({ model, requestCount }) => `${model}:${requestCount}`).join(",")}
              >
                <Cpu aria-hidden />
                <span>{activeRequestSummary}</span>
              </span>
            ) : null}
          </div>
        </div>
        <div className="pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <PoolSpeedControl
              key={mode}
              value={serviceTier}
              disabled={!supportsRoutingSettings}
              saving={pendingServiceTier !== null || busy === "pool-service-tier"}
              onChange={(value) => void updateServiceTier(value)}
            />
            <IconButton
              label={t("pool.routingSettings")}
              icon={<SlidersHorizontal aria-hidden />}
              disabled={!supportsRoutingSettings}
              title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined}
              onClick={onRoutingPolicy}
            />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            {hasAccountMembers ? (
              <IconButton
                className="account-calculation-toggle"
                label={t(accountValueVisible ? "pool.hideCalculation" : "pool.showCalculation")}
                icon={<DollarSign aria-hidden />}
                aria-pressed={accountValueVisible}
                onClick={() => setAccountValueVisible(!accountValueVisible)}
              />
            ) : null}
            <IconButton
              label={t("pool.refreshQuotas")}
              icon={busy === "pool-quota-refresh" ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
              aria-busy={busy === "pool-quota-refresh"}
              disabled={busy === "pool-quota-refresh" || !canRefreshQuota || !refreshableMemberCount}
              title={!refreshableMemberCount
                ? t("pool.noQuotaMembers")
                : !canRefreshQuota
                  ? t("remote.capabilityUnavailable")
                  : undefined}
              onClick={() => void refreshQuotas()}
            />
          </div>
        </div>
      </div>
    </div>
    {routingAlert}
    {quotaReport ? (
      <div className={`account-quota-report${quotaReport.failed ? " has-errors" : ""}`} role="status">
        <CheckCheck aria-hidden />
        <span>{t("accounts.quotaRefreshReport", quotaReport)}</span>
        <button type="button" aria-label={t("common.close")} onClick={() => setQuotaReport(null)}>
          <X aria-hidden />
        </button>
      </div>
    ) : null}
    <div className="pool-member-list" role="list" aria-label={t("pool.members")}>
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const runtimeState = runtimeByMember.get(member.id);
        const isCurrent = activeRequestCount(runtimeState) > 0;
        const isNext = !isCurrent && nextMember?.kind === member.kind && nextMember.id === member.id;
        const isLastUsed = !isCurrent && runtimeState != null && runtimeState.lastUsedAtMs != null && runtimeState.lastUsedAtMs === lastUsedRuntime?.lastUsedAtMs;
        return <PoolMemberCard
          key={`${member.kind}-${member.id}`}
          member={member}
          nowMs={nowMs}
          runtimeState={runtimeState}
          selected={selectedId === memberId}
          isNext={isNext}
          isLastUsed={isLastUsed}
          sourceState={member.kind === "source" ? sourceStats[member.id] : undefined}
          rotationMode={rotationMode}
          canRefreshQuota={canRefreshQuota}
          onShowError={setErrorDetails}
          onEdit={() => setSelectedId(memberId)}
          onRemove={() => void remove(member)}
          onConfirmRemove={() => void confirmRemove(member)}
          onRefreshSource={() => void refreshSourceStats(member.id, true)}
          onReauthenticate={onReauthenticate}
        />;
      })}
    </div>
    {selected ? <PoolMemberEditor key={`${selected.kind}:${selected.id}`} member={selected} onClose={() => setSelectedId(null)} /> : null}
    {errorDetails ? <AccountErrorDialog account={errorDetails} onClose={() => setErrorDetails(null)} /> : null}
  </>;
}
