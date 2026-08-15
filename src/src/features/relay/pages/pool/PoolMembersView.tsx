import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, CheckCheck, Clock3, Cloud, DollarSign, Gauge, ListMinus, Loader2, Pencil, RefreshCw, UserRound, X, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, SourceStats, SourceSummary } from "../../api/types";
import { currentAccountErrorCode, operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { AccountPlanBadge, Button, EmptyState, IconButton, ProviderQuotaStrip, QuotaEconomicsStrip, QuotaStack, StatusIcon, accountErrorLabel, formatDetailedRemainingTime, useConfirm } from "../../components/Ui";
import { activeModelCounts, activeRequestCount, apiSourceRole, routingOrderPositions } from "../../routingOrder";
import { comparePoolMembers, memberName, type PoolMember } from "../../poolHelpers";
import { formatApiEquivalent, formatProviderMicroUsd } from "../../poolFormatting";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayState } from "../../state/RelayStateProvider";
import { AccountErrorDialog } from "../connections/AccountsTable";

type Member = PoolMember;
type SourceStatsState = { value: SourceStats | null; loading: boolean; failed: boolean };

export function PoolMembersView({ onAdd, onRoutingPolicy, supportsRoutingSettings }: { onAdd: () => void; onRoutingPolicy: () => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy, codexPoolOauthSelection, accountEconomicsVisible, setAccountEconomicsVisible, accountQuotaCalculationMode } = useRelayState();
  const confirm = useConfirm();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const serviceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const routingStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
  const subscriptionExpiryFormat = new Intl.DateTimeFormat(i18n.language, { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [errorDetails, setErrorDetails] = useState<AccountSummary | null>(null);
  const [nowMs, setNowMs] = useState(Date.now());
  const [quotaReport, setQuotaReport] = useState<{ succeeded: number; failed: number } | null>(null);
  const [sourceStats, setSourceStats] = useState<Record<string, SourceStatsState>>({});
  const sourceStatsGeneration = useRef(0);
  const poolMembers: Member[] = [
    ...(runtime?.accounts ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "account" as const })),
    ...(runtime?.sources ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "source" as const })),
  ];
  const runtimeOrder = runtime?.gateway.routingOrder ?? [];
  const runtimeByMember = new Map(runtimeOrder.map((candidate) => [candidate.candidateId, candidate]));
  const orderByMember = routingOrderPositions(runtimeOrder);
  const members = [...poolMembers].sort((left, right) => comparePoolMembers(left, right, orderByMember));
  const sourceIds = members.filter((member) => member.kind === "source").map((member) => member.id).sort().join("\n");
  const refreshSourceStats = useCallback(async (sourceId: string) => {
    if (mode === "zenith") return;
    const generation = sourceStatsGeneration.current;
    setSourceStats((current) => ({
      ...current,
      [sourceId]: { value: current[sourceId]?.value ?? null, loading: true, failed: false },
    }));
    try {
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
  const upcomingTimes = members.flatMap((member) => [
    ...(member.kind === "account" ? [member.subscription.activeUntilMs, member.quota.primary?.resetAtMs, member.quota.secondary?.resetAtMs, ...(member.quota.supplemental ?? []).map((item) => item.window.resetAtMs)] : []),
    runtimeByMember.get(member.id)?.nextRetryAtMs,
  ].filter((value): value is number => value != null && value > nowMs));
  const showSeconds = upcomingTimes.some((value) => value - nowMs < 60 * 60_000);
  useEffect(() => {
    const timer = window.setTimeout(() => setNowMs(Date.now()), showSeconds ? 1_000 : 60_000);
    return () => window.clearTimeout(timer);
  }, [nowMs, showSeconds]);
  const activeMembers = members.filter((member) => activeRequestCount(runtimeByMember.get(member.id)) > 0);
  const activeRuntime = activeMembers.flatMap((member) => {
    const candidate = runtimeByMember.get(member.id);
    return candidate ? [candidate] : [];
  });
  const activeRequestTotal = activeRuntime.reduce((total, candidate) => total + activeRequestCount(candidate), 0);
  const activeModels = activeModelCounts(activeRuntime);
  const activeModelList = activeModels
    .map(({ model, requestCount }) => requestCount > 1 ? t("pool.activeModelCount", { model, count: requestCount }) : model)
    .join(" · ");
  const activeRequestSummary = activeRequestTotal > 0
    ? activeModelList
      ? t("pool.activeRequests", { count: activeRequestTotal, models: activeModelList })
      : t("pool.activeRequestsUnknown", { count: activeRequestTotal })
    : null;
  const lastUsedRuntime = runtimeOrder.reduce<CandidateRuntimeSnapshot | null>((latest, candidate) => candidate.lastUsedAtMs != null && (latest?.lastUsedAtMs == null || candidate.lastUsedAtMs > latest.lastUsedAtMs) ? candidate : latest, null);
  const lastUsedMember = lastUsedRuntime ? members.find((member) => member.id === lastUsedRuntime.candidateId) ?? null : null;
  const nextMember = members.find((member) => runtimeByMember.get(member.id)?.available) ?? null;
  const routingSummary = activeMembers.length === 1
    ? `${t("pool.currentRoute")}: ${memberName(activeMembers[0])}`
    : activeMembers.length > 1
      ? t("pool.activeRoutes", { count: activeMembers.length })
      : lastUsedMember
        ? `${t("pool.lastRoute")}: ${memberName(lastUsedMember)}`
        : nextMember
          ? `${t("pool.nextRoute")}: ${memberName(nextMember)}`
          : t("pool.priorityEmpty");
  const selected = members.find((member) => `${member.kind}:${member.id}` === selectedId) ?? null;
  const remove = async (member: Member) => {
    const ok = await perform(`pool-remove-${member.id}`, () => mode === "local"
      ? relayCommands.setPoolMembership(member.kind === "account" ? [member.id] : [], member.kind === "source" ? [member.id] : [], false)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: member.kind === "account" ? [member.id] : [], sourceIds: member.kind === "source" ? [member.id] : [], inPool: false }), "feedback.saved");
    if (ok) setSelectedId(null);
  };
  const confirmRemove = async (member: Member) => {
    const name = member.kind === "source" ? member.name : member.label;
    if (!await confirm(t("pool.removeMemberConfirm", { name }), { danger: true, confirmLabel: t("pool.removeMember") })) return;
    await remove(member);
  };
  const quotaAccountCount = members.filter((member) => member.kind === "account" && member.enabled).length;
  const hasAccountMembers = members.some((member) => member.kind === "account");
  const refreshQuotas = async () => {
    let report = { succeeded: 0, failed: 0 };
    const ok = await perform("pool-quota-refresh", async () => {
      if (mode === "local") {
        const results = await relayCommands.refreshAllAccountQuotas();
        report = {
          succeeded: results.filter((result) => result.status === "succeeded").length,
          failed: results.filter((result) => result.status === "failed").length,
        };
      } else {
        const result = await relayCommands.remoteAction({ type: "refresh_all_quotas" }) as { refreshed?: number; failed?: number };
        report = { succeeded: result.refreshed ?? 0, failed: result.failed ?? 0 };
      }
    });
    if (ok) setQuotaReport(report);
  };
  const refreshAccountQuota = (account: AccountSummary) => perform(
    `pool-account-quota-${account.id}`,
    () => mode === "local"
      ? relayCommands.refreshAccountQuota(account.id)
      : relayCommands.remoteAction({ type: "refresh_account", id: account.id }),
    "feedback.refreshed",
  );
  const updateServiceTier = (fast: boolean) => {
    const defaultServiceTier = fast ? "fast" : "standard";
    if (defaultServiceTier === serviceTier) return;
    void perform("pool-service-tier", () => persistRoutingPolicy(mode, {
      maxRetryCandidates: runtime?.gateway.maxRetryCandidates ?? 3,
      cooldownAfterFailures: runtime?.gateway.cooldownAfterFailures ?? 3,
      keepLastCandidateAvailable: runtime?.gateway.keepLastCandidateAvailable ?? true,
      routingStrategy,
      defaultServiceTier,
      subscriptionPlanOrder: runtime?.gateway.subscriptionPlanOrder ?? [],
    }));
  };
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={onAdd}>{t("pool.addMember")}</Button>} />;
  const statuses = members.map((member) => member.operationalStatus);
  const counts = {
    rotation: statuses.filter((status) => status === "rotation").length,
    quotaWait: statuses.filter((status) => status === "quotaWait").length,
    errors: members.filter((member) => member.kind === "account" ? Boolean(currentAccountErrorCode(member)) : member.operationalStatus === "unavailable").length,
    disabled: statuses.filter((status) => status === "disabled").length,
  };
  return <>
    <div className="pool-controls">
      <div className="table-toolbar pool-member-toolbar">
        <div className="pool-priority-label" title={t("pool.priorityHint")}><Activity aria-hidden /><span><strong>{t("pool.priorityTitle")}</strong><small>{routingSummary}</small>{activeRequestSummary ? <small className="pool-active-models" data-active-request-count={activeRequestTotal} data-active-models={activeModels.map(({ model, requestCount }) => `${model}:${requestCount}`).join(",")} title={activeRequestSummary}>{activeRequestSummary}</small> : null}</span></div>
        <div className="inline-actions pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <label className="pool-speed-control" data-fast={serviceTier === "fast" ? "true" : "false"} title={t("pool.serviceTierHint")}>
              <Zap aria-hidden />
              <span className="pool-speed-copy"><small>{t("pool.serviceTier")}</small><strong>{t(`pool.serviceTiers.${serviceTier}`)}</strong></span>
              <input type="checkbox" role="switch" aria-label={t("pool.serviceTier")} checked={serviceTier === "fast"} disabled={busy === "pool-service-tier"} onChange={(event) => updateServiceTier(event.target.checked)} />
              <span className="pool-speed-track" aria-hidden><span /></span>
            </label>
            <IconButton label={t("pool.routingSettings")} icon={<Gauge aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={onRoutingPolicy} />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            {hasAccountMembers ? <IconButton className="account-calculation-toggle" label={t(accountEconomicsVisible ? "pool.hideCalculation" : "pool.showCalculation")} icon={<DollarSign aria-hidden />} aria-pressed={accountEconomicsVisible} onClick={() => setAccountEconomicsVisible(!accountEconomicsVisible)} /> : null}
            <Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "pool-quota-refresh"} disabled={!canRefreshQuota || !quotaAccountCount} title={!quotaAccountCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()}>{t("pool.refreshQuotas")}</Button>
          </div>
        </div>
      </div>
      <div className="pool-summary"><div><span>{t("pool.memberStatus.rotation")}</span><strong>{counts.rotation}</strong></div><div><span>{t("pool.memberStatus.quotaWait")}</span><strong>{counts.quotaWait}</strong></div><div><span>{t("accounts.summary.errors")}</span><strong>{counts.errors}</strong></div><div><span>{t("pool.memberStatus.disabled")}</span><strong>{counts.disabled}</strong></div></div>
    </div>
    {quotaReport ? <div className={`account-quota-report${quotaReport.failed ? " has-errors" : ""}`} role="status"><CheckCheck aria-hidden /><span>{t("accounts.quotaRefreshReport", quotaReport)}</span><button type="button" aria-label={t("common.close")} onClick={() => setQuotaReport(null)}><X aria-hidden /></button></div> : null}
    <div className="pool-member-list" role="list" aria-label={t("pool.members")}>
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const runtimeState = runtimeByMember.get(member.id);
        const statusKey = member.operationalStatus;
        const statusTone = operationalStatusTone(statusKey);
        const runtimeTone = statusKey === "rotation"
          ? transientCandidateTone(runtimeState, nowMs, member.kind === "source")
          : null;
        const quotaStatus = member.kind === "account" ? member.quotaRefreshStatus : "updated";
        const errorCode = member.kind === "account" ? currentAccountErrorCode(member) : null;
        const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
        const indicatorTone = statusKey === "unavailable" || statusKey === "disabled"
          ? statusTone
          : quotaStatus === "refreshing"
            ? "disabled"
            : quotaStatus === "failed" || quotaStatus === "requires_reauth"
              ? "error"
              : quotaStatus === "pending"
                ? "disabled"
                : runtimeTone ?? statusTone;
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
        const isLastUsed = !isCurrent && runtimeState != null && runtimeState.candidateId === lastUsedRuntime?.candidateId && runtimeState.kind === lastUsedRuntime.kind;
        const runtimeHint = runtimeState?.halfOpen
          ? t("pool.recoveryProbe")
          : member.kind === "source" && runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
            ? t("pool.retryAt", { time: new Date(runtimeState.nextRetryAtMs).toLocaleString(i18n.language) })
            : undefined;
        const editLabel = `${t("pool.editMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removeLabel = `${t("pool.removeMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removing = busy === `pool-remove-${member.id}`;
        const statusLabel = t(`pool.memberStatus.${statusKey}`);
        const indicatorLabel = displayedErrorCode ? accountErrorLabel(displayedErrorCode, t) : quotaStatus === "updated" ? statusLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${statusLabel}`;
        const indicatorHint = runtimeHint ? `${indicatorLabel} · ${runtimeHint}` : indicatorLabel;
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}${isCurrent ? " current" : ""}`} role="listitem" title={codexInterface ? t("pool.codexInterfaceHint") : undefined} data-member-label={member.kind === "source" ? member.name : member.label} data-current={isCurrent ? "true" : "false"} data-last-used={isLastUsed ? "true" : "false"} data-member-kind={member.kind}>
          <header className="pool-member-card-header">
            {member.kind === "account" && displayedErrorCode
              ? <IconButton className="pool-member-kind-icon" data-status="error" label={indicatorHint} icon={<UserRound aria-hidden />} onClick={() => setErrorDetails(member)} />
              : <StatusIcon className="pool-member-kind-icon" status={indicatorTone} label={indicatorHint}>{member.kind === "source" ? <Cloud aria-hidden /> : <UserRound aria-hidden />}</StatusIcon>}
            <div className="pool-member-identity">
              <strong className="pool-member-name" title={identity === detail ? identity : `${identity} · ${detail}`}>{identity}</strong>
              <div className="pool-member-meta">{member.kind === "account" ? <AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} /> : <small title={detail}>{detail}</small>}</div>
            </div>
          </header>
          <div className={`pool-member-card-quota${member.kind === "account" ? " compact-quota-layout" : ""}`}>{member.kind === "account" ? <PoolAccountQuota account={member} nowMs={nowMs} /> : <PoolSourceStats source={member} state={sourceStats[member.id]} />}</div>
          <div className="pool-member-context" data-kind={member.kind}>{member.kind === "account" ? <><span className="pool-member-subscription-date">{subscriptionExpiry?.date}</span>{subscriptionExpiry?.remaining ? <><span className="pool-member-context-separator" aria-hidden>·</span><span className="pool-member-subscription-expiry">{subscriptionExpiry.remaining}</span></> : null}</> : <span>{t(`sources.roles.${apiSourceRole(member.priority)}`)}</span>}</div>
          {member.kind === "account" && accountEconomicsVisible ? accountQuotaCalculationMode === "provider" ? <ProviderQuotaStrip account={member} nowMs={nowMs} /> : <QuotaEconomicsStrip account={member} /> : null}
          <footer className="pool-member-card-footer" data-kind={member.kind}>
            <div className="pool-member-actions">
              <IconButton className="danger" data-relay-context-action label={removeLabel} icon={removing ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />} disabled={removing} onClick={() => void confirmRemove(member)} onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                void remove(member);
              }} />
              {member.kind === "source" ? <IconButton label={t("pool.refreshSourceStats")} icon={sourceStats[member.id]?.loading ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={!member.secretAvailable || sourceStats[member.id]?.loading} onClick={() => void refreshSourceStats(member.id)} /> : null}
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

function PoolAccountQuota({ account, nowMs }: { account: AccountSummary; nowMs: number }) {
  const { t } = useTranslation();
  const hasQuota = Boolean(account.quota.primary || account.quota.secondary || account.quota.supplemental?.length);
  const status = account.quotaRefreshStatus ?? (account.authState.state === "requires_reauth" ? "requires_reauth" : account.quota.error ? "failed" : account.quota.updatedAtMs != null ? "updated" : "pending");
  return <>{!hasQuota ? <div className={`account-quota-refresh-state ${status}`} role="status">{status === "refreshing" ? <Loader2 className="spin" aria-hidden /> : status === "updated" ? <CheckCheck aria-hidden /> : status === "requires_reauth" ? <UserRound aria-hidden /> : status === "failed" ? <RefreshCw aria-hidden /> : <Clock3 aria-hidden />}<span>{t(`accounts.quotaRefreshStatus.${status}`)}</span></div> : <QuotaStack snapshot={account.quota} nowMs={nowMs} concise />}</>;
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
    ? stats.requests == null ? "—" : new Intl.NumberFormat(locale).format(stats.requests)
    : "—";
  return <dl className="pool-source-stats">
    <div title={state?.failed ? t("overview.sourceStatsUnavailable") : !providerStats && !state?.loading ? t("overview.sourceStatsUnsupported") : undefined}><dt>{t("overview.balance")}</dt><dd data-muted={!providerStats ? "true" : undefined}>{balance}</dd></div>
    <div title={!providerStats ? t("pool.apiEquivalentHint", { count: source.apiEquivalent.unpricedTokens }) : undefined}><dt>{providerStats ? t("overview.spent") : t("pool.apiEquivalent")}</dt><dd>{spent}</dd></div>
    <div><dt>{t("usage.requests")}</dt><dd>{requests}</dd></div>
    <div><dt>{t("common.models")}</dt><dd>{new Intl.NumberFormat(locale).format(source.models.length)}</dd></div>
  </dl>;
}
