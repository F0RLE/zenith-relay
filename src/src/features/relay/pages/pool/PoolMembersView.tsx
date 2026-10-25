import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, CircleAlert, CircleCheck, CirclePause, Clock3, Coins, Cpu, DollarSign, Loader2, RefreshCw, SlidersHorizontal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, DefaultServiceTier } from "../../api/types";
import { refreshAllAccountQuotas } from "../../accountQuotaRefresh";
import { useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { accountMetadataObservation } from "../../accountRoutes";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { Button, EmptyState, IconButton, useConfirm } from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { activeRequestCount } from "../../routingOrder";
import { memberName, type PoolMember } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useSourceStats } from "../../hooks/useSourceStats";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayActivity, useRelayState } from "../../state/relayStateContext";
import { setVisibleIdsSelected, toggleSelectedId } from "../../state/selectionModel";
import { AccountErrorDialog } from "../connections/AccountErrorDialog";
import { PoolSpeedControl } from "./PoolSpeedControl";
import { PoolMemberCard } from "./PoolMemberCard";
import { PoolEconomicsStrip } from "./PoolEconomicsStrip";
import { PoolSelectionActions } from "./PoolSelectionActions";
import { poolMemberKey } from "./poolSelectionModel";
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

export function PoolMembersView({ onAdd, onRoutingPolicy, onReauthenticate, supportsRoutingSettings }: { onAdd: () => void; onRoutingPolicy: () => void; onReauthenticate: (account: AccountSummary, automatic?: boolean) => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy, accountValueVisible, setAccountValueVisible } = useRelayState();
  const runtimeActivity = useRelayActivity();
  const confirm = useConfirm();
  const canAdd = mode === "local";
  const canRefreshQuota = mode === "local";
  const [pendingServiceTier, setPendingServiceTier] = useState<DefaultServiceTier | null>(null);
  const serviceTier = pendingServiceTier ?? runtime?.gateway.defaultServiceTier ?? "standard";
  useEffect(() => {
    if (pendingServiceTier !== null && pendingServiceTier === runtime?.gateway.defaultServiceTier) {
      setPendingServiceTier(null);
    }
  }, [pendingServiceTier, runtime?.gateway.defaultServiceTier]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const selectAllRef = useRef<HTMLInputElement>(null);
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const poolMembers: Member[] = useMemo(
    () => runtime ? poolMembersFromRuntime(runtime) : EMPTY_POOL_MEMBERS,
    [runtime?.accounts, runtime?.sources],
  );
  const runtimeOrder = runtime?.gateway.routingOrder ?? EMPTY_RUNTIME_ORDER;
  const runtimeByMember = useMemo(
    () => poolMemberRuntimeStates(poolMembers, runtimeOrder, runtimeActivity),
    [poolMembers, runtimeActivity, runtimeOrder],
  );
  const savedRoutingMembers = runtime?.gateway.poolRouting?.members;
  const rotationMode = runtime?.gateway.poolRouting && runtime.gateway.poolRouting.version !== 1 ? runtime.gateway.poolRouting.mode : null;
  const routingTitle = t(rotationMode === "automatic" ? "pool.distributionTitle" : rotationMode === "failover" ? "pool.failoverTitle" : "pool.priorityTitle");
  const members = useMemo(
    () => orderedPoolMembers(poolMembers, runtimeOrder, savedRoutingMembers, rotationMode, runtimeByMember),
    [poolMembers, rotationMode, runtimeOrder, savedRoutingMembers, runtimeByMember],
  );
  const selectedMembers = members.filter((member) => selectedIds.includes(poolMemberKey(member)));
  const allSelected = members.length > 0 && selectedMembers.length === members.length;
  useEffect(() => {
    const keys = new Set(poolMembers.map(poolMemberKey));
    setSelectedIds((previous) => previous.filter((id) => keys.has(id)));
  }, [poolMembers]);
  useEffect(() => {
    setSelectedIds([]);
    setSelectedId(null);
  }, [mode]);
  useEffect(() => {
    if (selectAllRef.current) selectAllRef.current.indeterminate = selectedMembers.length > 0 && !allSelected;
  }, [selectedMembers.length, allSelected]);
  const providerCredits = useMemo(() => poolProviderCreditsSummary(members), [members]);
  const providerCreditsValue = providerCredits == null
    ? null
    : providerCredits.kind === "unlimited"
      ? "∞"
      : formatNumber(providerCredits.availableCredits, i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 1 });
  const visibleModelIds = runtime?.gateway.visibleModelIds ?? EMPTY_VISIBLE_MODELS;
  // Statistics follow inventory facts, not the activity-driven display order.
  const sourceMembers = useMemo(() => poolMembers.filter((member): member is Extract<Member, { kind: "source" }> => member.kind === "source"), [poolMembers]);
  const { stats: sourceStats, refresh: readSourceStats } = useSourceStats(mode, sourceMembers);
  const memberTimestamps = useMemo(() => members.flatMap((member) => [
    ...(member.kind === "account" ? [
      member.subscription.activeUntilMs,
      !member.recoveryRequired ? member.credentialDiagnostics?.expiresAtMs : null,
      member.quota.primary?.resetAtMs,
      member.quota.secondary?.resetAtMs,
      ...(member.quota.supplemental ?? []).map((quotaWindow) => quotaWindow.window.resetAtMs),
    ] : []),
    runtimeByMember.get(member.id)?.nextRetryAtMs,
  ]).concat((runtime?.accounts ?? []).flatMap((account) => [
    accountMetadataObservation(account, "plan").staleAtMs,
    accountMetadataObservation(account, "quota").staleAtMs,
    account.subscription.activeUntilMs,
    account.quota.primary?.resetAtMs,
    account.quota.secondary?.resetAtMs,
    ...(account.quota.supplemental ?? []).map((window) => window.window.resetAtMs),
  ])), [members, runtimeByMember, runtime?.accounts]);
  const nowMs = useRelativeTimeClock(memberTimestamps);
  const refreshSourceStats = async (sourceId: string, refreshModels = false, operationManaged = false) => {
    let modelRefreshError: unknown;
    if (refreshModels && mode === "local") {
      const refreshModels = () => relayCommands.refreshSourceData(sourceId);
      if (operationManaged) {
        try { await refreshModels(); } catch (error) { modelRefreshError = error; }
      } else await perform(`source-data-refresh-${sourceId}`, refreshModels, "feedback.refreshed", { backgroundRefresh: true });
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
  const counts = poolMemberStatusCounts(members, runtimeByMember);
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
  const selected = members.find((member) => `${member.kind}:${member.id}` === selectedId) ?? null;
  const remove = async (member: Member) => {
    const ok = await perform(`pool-remove-${member.id}`, () => updatePoolMembership(mode, {
      accountIds: member.kind === "account" ? [member.id] : [],
      sourceIds: member.kind === "source" ? [member.id] : [],
      inPool: false,
    }), "feedback.saved", { backgroundRefresh: true });
    if (ok) setSelectedId(null);
  };
  const confirmRemove = async (member: Member) => {
    const memberName = member.kind === "source" ? member.name : member.label;
    if (!await confirm(t("pool.removeMemberConfirm", { name: memberName }), { danger: true, confirmLabel: t("pool.removeMember") })) return;
    await remove(member);
  };
  const quotaAccountCount = members.filter((member) => member.kind === "account" && member.enabled).length;
  const refreshableSourceIds = members
    .filter((member): member is Extract<Member, { kind: "source" }> => member.kind === "source" && member.secretAvailable)
    .map((member) => member.id);
  const refreshableMemberCount = quotaAccountCount + refreshableSourceIds.length;
  const hasAccountMembers = members.some((member) => member.kind === "account");
  const refreshQuotas = async () => {
    await perform("pool-quota-refresh", async () => {
      if (quotaAccountCount) await refreshAllAccountQuotas(mode);
      await Promise.all(refreshableSourceIds.map((sourceId) => refreshSourceStats(sourceId, mode === "local", true)));
    }, undefined, { backgroundRefresh: true });
  };
  const updateServiceTier = async (defaultServiceTier: DefaultServiceTier) => {
    if (defaultServiceTier === serviceTier) return;
    setPendingServiceTier(defaultServiceTier);
    const ok = await perform("pool-service-tier", () => persistRoutingPolicy(mode, {
      maxRetryCandidates: runtime?.gateway.maxRetryCandidates ?? 3,
      defaultServiceTier,
    }), "feedback.saved", { backgroundRefresh: true, uiLock: false });
    if (!ok) setPendingServiceTier((previousServiceTier) => previousServiceTier === defaultServiceTier ? null : previousServiceTier);
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
            title={!canAdd ? t("common.unsupported") : undefined}
            onClick={onAdd}
          >
            {t("pool.addMember")}
          </Button>
        )}
      />
    );
  }
  return <>
    <div className="pool-controls workspace-controls" role="group" aria-label={routingTitle}>
      <div className="pool-summary-band">
        <div className="pool-summary relay-status-summary" data-has-provider-credits={providerCreditsValue != null ? "true" : "false"}>
          <div role="group" tabIndex={0} aria-label={t("pool.memberStatus.rotation")} data-relay-tooltip={t("pool.memberStatus.rotation")} data-tone={counts.rotation ? "ready" : "muted"}><CircleCheck aria-hidden /><strong>{counts.rotation}</strong></div>
          <div role="group" tabIndex={0} aria-label={t("pool.waiting")} data-relay-tooltip={t("pool.waiting")} data-tone={counts.waiting ? "warning" : "muted"}><Clock3 aria-hidden /><strong>{counts.waiting}</strong></div>
          <div role="group" tabIndex={0} aria-label={t("accounts.summary.errors")} data-relay-tooltip={t("accounts.summary.errors")} data-tone={counts.errors ? "error" : "muted"}><CircleAlert aria-hidden /><strong>{counts.errors}</strong></div>
          <div role="group" tabIndex={0} aria-label={t("pool.memberStatus.disabled")} data-relay-tooltip={t("pool.memberStatus.disabled")} data-tone="muted"><CirclePause aria-hidden /><strong>{counts.disabled}</strong></div>
          {providerCreditsValue != null ? (
            <div role="group" tabIndex={0} aria-label={t("pool.totalProviderCredits")} data-summary="provider-credits" data-relay-tooltip={t("pool.totalProviderCreditsHint")}>
              <Coins aria-hidden />
              <strong>{providerCreditsValue}</strong>
            </div>
          ) : null}
        </div>
        <PoolEconomicsStrip members={poolMembers} />
      </div>
      <div className="pool-member-toolbar" data-selection={selectedMembers.length > 0}>
        <div className="pool-selection-context">
          <input ref={selectAllRef} type="checkbox" checked={allSelected} disabled={Boolean(busy)}
            aria-label={t("pool.selectAll")}
            onChange={(event) => setSelectedIds((ids) => setVisibleIdsSelected(ids, members.map(poolMemberKey), event.target.checked))} />
          {selectedMembers.length ? <span className="pool-selected-count" role="status"
            aria-label={t("pool.selectionCount", { count: selectedMembers.length })}
            data-relay-tooltip={t("pool.selectionCount", { count: selectedMembers.length })}>
            <span className="pool-selected-count-full">{t("pool.selectionCount", { count: selectedMembers.length })}</span>
            <span className="pool-selected-count-short" aria-hidden>{selectedMembers.length}</span>
          </span> : <div className="pool-priority-context">
          <div className="pool-priority-label" data-relay-tooltip={t(rotationMode === "automatic" ? "pool.priorityHintAutomatic" : rotationMode === "failover" ? "pool.priorityHintFailover" : "pool.priorityHint")}><h2>{routingTitle}</h2></div>
          <div className="pool-runtime-strip">
            <div className="pool-route-summary">
              <strong className="pool-current-route" data-active={activeRequestTotal > 0} data-relay-tooltip={routingSummary}>{routingSummary}</strong>
              {nextRouteSummary ? <span className="pool-next-route" data-relay-tooltip={nextRouteSummary}><ArrowRight aria-hidden /><span>{nextRouteSummary}</span></span> : null}
            </div>
            <span
              className="pool-active-models"
              aria-hidden={!activeRequestSummary}
              data-active-request-count={activeRequestTotal}
              data-active-models={activeModels.map(({ model, requestCount }) => `${model}:${requestCount}`).join(",")}
              data-relay-tooltip={activeRequestSummary ?? undefined}
            >
              <Cpu aria-hidden />
              <span>{activeRequestSummary}</span>
            </span>
          </div>
        </div>}
        </div>
        {selectedMembers.length ? <PoolSelectionActions
          members={selectedMembers}
          onClear={() => setSelectedIds([])}
          onRemoved={(removed) => {
            const keys = new Set(removed.map(poolMemberKey));
            setSelectedIds((ids) => ids.filter((id) => !keys.has(id)));
            setSelectedId((id) => id && keys.has(id) ? null : id);
          }}
          onRefreshSource={(id) => refreshSourceStats(id, true, true)}
        /> : <div className="pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <PoolSpeedControl
              key={mode}
              value={serviceTier}
              disabled={!supportsRoutingSettings}
              saving={pendingServiceTier !== null || busy === "pool-service-tier"}
              onChange={(serviceTier) => void updateServiceTier(serviceTier)}
            />
            <IconButton
              label={t("pool.routingSettings")}
              icon={<SlidersHorizontal aria-hidden />}
              disabled={!supportsRoutingSettings}
              title={!supportsRoutingSettings ? t("common.unsupported") : undefined}
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
                  ? t("common.unsupported")
                  : undefined}
              onClick={() => void refreshQuotas()}
            />
          </div>
        </div>}
      </div>
    </div>
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
          checked={selectedIds.includes(memberId)}
          onToggleSelected={() => setSelectedIds((ids) => toggleSelectedId(ids, memberId))}
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
