import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { Activity, ArrowDown, ArrowRightLeft, ArrowUp, BrainCircuit, CheckCheck, Clock3, Cloud, DollarSign, Download, Gauge, GripVertical, ListMinus, Loader2, Pencil, Play, Plus, Power, RefreshCw, RotateCcw, Trash2, Upload, UserRound, X, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, ConfigurationPresetPreview, DefaultServiceTier, ModelSummary, RelayMode, RoutingStrategy, SourceStats, SourceSummary } from "../../api/types";
import { PoolMemberEditor } from "../../components/PoolMemberEditor";
import { QuotaEconomicsStrip, AccountPlanBadge, Button, Dialog, EmptyState, IconButton, OptionMenu, PageHeader, QuotaStack, StatusIcon, Tabs, accountErrorLabel, currentAccountErrorCode, formatDetailedRemainingTime, isCodexOauthAccountEligible, operationalStatusTone, transientCandidateTone, useConfirm } from "../../components/Ui";
import { supportsCacheWritePricing } from "../../modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../../modelPricing";
import { accountPlanOption, apiSourceRole, compareAccountPlans, activeModelCounts, activeRequestCount, compareRoutingOrder, routingOrderPositions } from "../../routingOrder";
import { clampRoutingCount, comparePoolMembers, compareStableText, groupModelSummariesForLauncher, memberName, mergeSubscriptionPlanOrder, modelSummaries, subscriptionPlanGroups, toggle, type PoolMember } from "../../poolHelpers";
import { useRelayState } from "../../state/RelayStateProvider";
import { AccountErrorDialog, SourceDialog } from "../connections/ConnectionsPage";

type View = "members" | "models";
type Member = PoolMember;
type SourceStatsState = { value: SourceStats | null; loading: boolean; failed: boolean };

export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, activateCodexProfile, busy, perform, codexPoolOauthSelection } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [routingPolicy, setRoutingPolicy] = useState(false);
  const [configurationPreview, setConfigurationPreview] = useState<ConfigurationPresetPreview | null>(null);
  const supportsModels = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("models"));
  const supportsMembers = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const supportsRoutingSettings = Boolean(runtime);
  const supportsConfigurationPresets = mode === "remote" && Boolean(runtime?.capabilities.features.includes("configuration_presets"));
  const canSaveConfigurationPreset = mode === "local" || supportsConfigurationPresets;
  useEffect(() => {
    if (view === "models" && !supportsModels) setView("members");
  }, [view, supportsModels]);
  const selectedOauthAccountId = codexPoolOauthSelection !== "none" && codexPoolOauthSelection !== "auto"
    && runtime?.accounts.some((account) => account.id === codexPoolOauthSelection && isCodexOauthAccountEligible(account))
    ? codexPoolOauthSelection
    : null;
  const switchCodexToPool = () => activateCodexProfile(
    "pool-switch",
    () => relayCommands.attachCodexGateway(selectedOauthAccountId, codexPoolOauthSelection === "none"),
    true,
  );
  const running = Boolean(runtime?.gateway.running);
  const exportConfiguration = () => perform("configuration-preset-export", mode === "local" ? relayCommands.exportLocalConfigurationPreset : relayCommands.exportRemoteConfigurationPreset);
  const previewConfiguration = () => perform("configuration-preset-preview", async () => {
    const preview = await relayCommands.previewRemoteConfigurationPreset();
    if (preview) setConfigurationPreview(preview);
  });
  const poolToggleLabel = running ? t("pool.stop") : t("pool.start");
  const poolToggleShortLabel = running ? t("pool.stopShort") : t("pool.startShort");
  const action = <div className="pool-header-actions">
    {canSaveConfigurationPreset ? <div className="pool-preset-actions">
      <Button variant="secondary" icon={<Download aria-hidden />} aria-label={t("pool.exportConfiguration")} title={t("pool.exportConfiguration")} disabled={Boolean(busy)} busy={busy === "configuration-preset-export"} onClick={() => void exportConfiguration()}>{t("pool.exportConfigurationShort")}</Button>
      {supportsConfigurationPresets ? <Button variant="secondary" icon={<Upload aria-hidden />} aria-label={t("pool.importConfiguration")} title={t("pool.importConfiguration")} disabled={Boolean(busy)} busy={busy === "configuration-preset-preview"} onClick={() => void previewConfiguration()}>{t("pool.importConfigurationShort")}</Button> : null}
    </div> : null}
    {view === "members" ? <Button data-action="pool-add" variant="secondary" icon={<Plus aria-hidden />} aria-label={t("pool.addMember")} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : t("pool.addMember")} onClick={() => setAddMembers(true)}>{t("pool.addMemberShort")}</Button> : null}
    {mode === "local" ? <>
      <Button data-action="pool-toggle" variant="secondary" icon={running ? <Power aria-hidden /> : <Play aria-hidden />} aria-label={poolToggleLabel} busy={busy === "pool-toggle"} title={poolToggleLabel} onClick={() => void perform("pool-toggle", running ? relayCommands.stopGateway : relayCommands.startGateway, running ? "feedback.stopped" : "feedback.started")}>{poolToggleShortLabel}</Button>
      <Button data-action="pool-switch" variant="primary" icon={<ArrowRightLeft aria-hidden />} aria-label={t("pool.switchChatGPT")} busy={busy === "pool-switch"} disabled={!running} title={!running ? t("pool.start") : t("pool.switchChatGPT")} onClick={() => void switchCodexToPool()}>{t("pool.switchChatGPTShort")}</Button>
    </> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <MembersView onAdd={() => setAddMembers(true)} onRoutingPolicy={() => setRoutingPolicy(true)} supportsRoutingSettings={supportsRoutingSettings} /> : null}{view === "models" ? <ModelsView /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{configurationPreview ? <ConfigurationPresetDialog preview={configurationPreview} onClose={() => setConfigurationPreview(null)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function ConfigurationPresetDialog({ preview, onClose }: { preview: ConfigurationPresetPreview; onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const apply = async () => {
    if (!preview.changes.length) return onClose();
    if (await perform("configuration-preset-apply", () => relayCommands.applyRemoteConfigurationPreset(preview), "feedback.saved")) onClose();
  };
  return <Dialog wide title={t("pool.configurationPreset")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" icon={<Upload aria-hidden />} busy={busy === "configuration-preset-apply"} disabled={!preview.changes.length} onClick={() => void apply()}>{t("pool.applyConfiguration")}</Button></>}>
    <div className="configuration-preset-preview">
      <header><strong>{t("pool.configurationChanges", { count: preview.changes.length })}</strong><code title={preview.baseRevision}>{preview.baseRevision.slice(0, 16)}</code></header>
      {preview.changes.length ? <div className="table-wrap"><table><thead><tr><th>{t("pool.configurationSetting")}</th><th>{t("pool.configurationCurrent")}</th><th>{t("pool.configurationNext")}</th></tr></thead><tbody>{preview.changes.map((change) => <tr key={change.path}><th scope="row"><code>{formatConfigurationPath(change.path)}</code></th><td><code>{formatConfigurationValue(change.before)}</code></td><td><code>{formatConfigurationValue(change.after)}</code></td></tr>)}</tbody></table></div> : <EmptyState title={t("pool.configurationUnchanged")} description={t("pool.configurationUnchangedHint")} />}
    </div>
  </Dialog>;
}

function formatConfigurationPath(path: string) {
  return path.split("/").filter(Boolean).join(" / ");
}

function formatConfigurationValue(value: unknown) {
  if (typeof value === "string") return value;
  return JSON.stringify(value) ?? String(value);
}

function MembersView({ onAdd, onRoutingPolicy, supportsRoutingSettings }: { onAdd: () => void; onRoutingPolicy: () => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy, codexPoolOauthSelection, accountEconomicsVisible, setAccountEconomicsVisible } = useRelayState();
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
            <IconButton className="pool-economics-toggle" label={t(accountEconomicsVisible ? "pool.hideEconomics" : "pool.showEconomics")} icon={<DollarSign aria-hidden />} aria-pressed={accountEconomicsVisible} onClick={() => setAccountEconomicsVisible(!accountEconomicsVisible)} />
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
          {member.kind === "account" && accountEconomicsVisible ? <QuotaEconomicsStrip account={member} /> : null}
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

function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const planGroups = subscriptionPlanGroups(runtime?.accounts ?? [], t("common.unknown"));
  const defaultPlanOrder = planGroups.map((group) => group.id);
  const storedPlanOrder = runtime?.gateway.subscriptionPlanOrder ?? [];
  const initialPlanOrder = mergeSubscriptionPlanOrder(planGroups, storedPlanOrder);
  const initialStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
  const [routingStrategy, setRoutingStrategy] = useState<RoutingStrategy>(initialStrategy);
  const defaultServiceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const [subscriptionPlanOrder, setSubscriptionPlanOrder] = useState(initialPlanOrder);
  const [draggedPlan, setDraggedPlan] = useState<string | null>(null);
  const [maxRetryCandidates, setMaxRetryCandidates] = useState(runtime?.gateway.maxRetryCandidates ?? 3);
  const [cooldownAfterFailures, setCooldownAfterFailures] = useState(runtime?.gateway.cooldownAfterFailures ?? 3);
  const [keepLastCandidateAvailable, setKeepLastCandidateAvailable] = useState(runtime?.gateway.keepLastCandidateAvailable ?? true);
  const hasCustomPlanOrder = subscriptionPlanOrder.length !== defaultPlanOrder.length || subscriptionPlanOrder.some((plan, index) => plan !== defaultPlanOrder[index]);
  const movePlan = (plan: string, target: string, after = false) => {
    if (plan === target) return;
    setSubscriptionPlanOrder((current) => {
      const next = current.filter((value) => value !== plan);
      const targetIndex = next.indexOf(target);
      if (targetIndex < 0) return current;
      next.splice(targetIndex + (after ? 1 : 0), 0, plan);
      return next;
    });
  };
  const movePlanBy = (plan: string, offset: number) => {
    const index = subscriptionPlanOrder.indexOf(plan);
    const target = subscriptionPlanOrder[index + offset];
    if (target) movePlan(plan, target, offset > 0);
  };
  const chooseStrategy = (value: string) => {
    setRoutingStrategy(value as RoutingStrategy);
  };
  const resetPlanOrder = () => {
    setSubscriptionPlanOrder(defaultPlanOrder);
  };
  const save = async () => {
    const savedPlanOrder = routingStrategy === "subscription_plan" ? subscriptionPlanOrder : [];
    const payload = {
      maxRetryCandidates,
      cooldownAfterFailures,
      keepLastCandidateAvailable,
      routingStrategy,
      defaultServiceTier,
      subscriptionPlanOrder: savedPlanOrder,
    };
    const ok = await perform("routing-policy", () => persistRoutingPolicy(mode, payload), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "routing-policy"} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form pool-policy-form">
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.routingStrategy")}</strong><small>{t(`pool.routingStrategyHints.${routingStrategy}`)}</small></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.routingStrategy")} value={routingStrategy} onChange={chooseStrategy} options={[{ value: "adaptive", label: t("pool.routingStrategies.adaptive") }, { value: "quota_highest", label: t("pool.routingStrategies.quotaHighest") }, { value: "subscription_expiry", label: t("pool.routingStrategies.subscriptionExpiry") }, { value: "subscription_plan", label: t("pool.routingStrategies.subscriptionPlan") }]} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.maxRetryCandidates")}</strong><small>{t("pool.maxRetryCandidatesHint")}</small></div>
        <input className="pool-policy-control" aria-label={t("pool.maxRetryCandidates")} type="number" min={1} max={8} value={maxRetryCandidates} onChange={(event) => setMaxRetryCandidates(clampRoutingCount(event.target.value))} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.cooldownAfterFailures")}</strong><small>{t("pool.cooldownAfterFailuresHint")}</small></div>
        <input className="pool-policy-control" aria-label={t("pool.cooldownAfterFailures")} type="number" min={1} max={8} value={cooldownAfterFailures} onChange={(event) => setCooldownAfterFailures(clampRoutingCount(event.target.value))} />
      </div>
      <label className="pool-policy-toggle toggle-row"><input type="checkbox" checked={keepLastCandidateAvailable} onChange={(event) => setKeepLastCandidateAvailable(event.target.checked)} /><span>{t("pool.keepLastCandidateAvailable")}</span></label>
      {routingStrategy === "subscription_plan" ? <div className="subscription-plan-policy">
        <div className="subscription-plan-policy-heading"><div><strong>{t("pool.subscriptionPlanOrder")}</strong><small>{t("pool.subscriptionPlanOrderHint")}</small></div>{hasCustomPlanOrder ? <IconButton label={t("pool.resetSubscriptionPlanOrder")} icon={<RotateCcw aria-hidden />} onClick={resetPlanOrder} /> : null}</div>
        {subscriptionPlanOrder.length ? <div className="subscription-plan-order" role="list" aria-label={t("pool.subscriptionPlanOrder")}>{subscriptionPlanOrder.map((plan, index) => {
          const group = planGroups.find((candidate) => candidate.id === plan);
          if (!group) return null;
          const drop = (event: DragEvent<HTMLDivElement>) => {
            event.preventDefault();
            if (draggedPlan) movePlan(draggedPlan, plan, subscriptionPlanOrder.indexOf(draggedPlan) < index);
            setDraggedPlan(null);
          };
          return <div key={plan} className="subscription-plan-order-row" role="listitem" draggable onDragStart={() => setDraggedPlan(plan)} onDragEnd={() => setDraggedPlan(null)} onDragOver={(event) => event.preventDefault()} onDrop={drop} data-subscription-plan={plan} data-dragging={draggedPlan === plan ? "true" : "false"}>
            <GripVertical aria-hidden />
            <span className="subscription-plan-rank">{index + 1}</span>
            <AccountPlanBadge planType={plan === "unknown" ? null : plan} unknown={t("common.unknown")} />
            <small>{t("pool.subscriptionPlanAccountCount", { count: group.count })}</small>
            <div className="inline-actions"><IconButton label={t("pool.moveSubscriptionPlanUp", { plan: group.label })} icon={<ArrowUp aria-hidden />} disabled={index === 0} onClick={() => movePlanBy(plan, -1)} /><IconButton label={t("pool.moveSubscriptionPlanDown", { plan: group.label })} icon={<ArrowDown aria-hidden />} disabled={index === subscriptionPlanOrder.length - 1} onClick={() => movePlanBy(plan, 1)} /></div>
          </div>;
        })}</div> : <p className="form-note">{t("pool.noSubscriptionPlanGroups")}</p>}
      </div> : null}
    </div>
  </Dialog>;
}

function AddMembersDialog({ onClose, onAddSource }: { onClose: () => void; onAddSource: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canAddSource = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("sources"));
  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [sourceIds, setSourceIds] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [planFilter, setPlanFilter] = useState("all");
  const allAccounts = (runtime?.accounts ?? []).filter((account) => !account.inPool);
  const planOptions = new Map<string, { id: string; label: string; count: number }>();
  for (const account of allAccounts) {
    const option = accountPlanOption(account.subscription.planType, t("common.unknown"));
    const current = planOptions.get(option.id);
    planOptions.set(option.id, { ...option, count: (current?.count ?? 0) + 1 });
  }
  const plans = [...planOptions.values()].sort(compareAccountPlans);
  const activePlan = planFilter === "all" || planOptions.has(planFilter) ? planFilter : "all";
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const accounts = allAccounts
    .filter((account) => activePlan === "all" || accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan)
    .filter((account) => !normalizedQuery || [account.identityHint, account.label, account.subscription.planType].some((value) => value?.toLocaleLowerCase().includes(normalizedQuery)))
    .sort((left, right) => compareAccountPlans(accountPlanOption(left.subscription.planType, t("common.unknown")), accountPlanOption(right.subscription.planType, t("common.unknown"))) || compareStableText(left.label, right.label));
  const sources = (runtime?.sources ?? []).filter((source) => !source.inPool);
  const selectedCount = accountIds.length + sourceIds.length;
  const availableCount = allAccounts.length + sources.length;
  const allSelected = availableCount > 0 && accountIds.length === allAccounts.length && sourceIds.length === sources.length;
  const shownSelected = accounts.length > 0 && accounts.every((account) => accountIds.includes(account.id));
  const toggleAll = () => {
    setAccountIds(allSelected ? [] : allAccounts.map((account) => account.id));
    setSourceIds(allSelected ? [] : sources.map((source) => source.id));
  };
  const toggleShown = () => setAccountIds(shownSelected
    ? accountIds.filter((id) => !accounts.some((account) => account.id === id))
    : [...new Set([...accountIds, ...accounts.map((account) => account.id)])]);
  const add = async () => {
    const ok = await perform("pool-add-members", () => mode === "local"
      ? relayCommands.setPoolMembership(accountIds, sourceIds, true)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds, sourceIds, inPool: true }), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide title={t("pool.addMembersTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "pool-add-members"} disabled={!selectedCount} onClick={add}>{t("pool.addSelected", { count: selectedCount })}</Button></>}>
    <div className="relay-form pool-member-picker">
      <div className="pool-member-picker-intro"><p className="form-note">{t("pool.addMembersHint")}</p><div className="inline-actions">{availableCount ? <Button variant="secondary" icon={allSelected ? <X aria-hidden /> : <CheckCheck aria-hidden />} onClick={toggleAll}>{allSelected ? t("accounts.clearSelection") : t("pool.selectAllMembers", { count: availableCount })}</Button> : null}<Button variant="secondary" icon={<Plus aria-hidden />} disabled={!canAddSource} title={!canAddSource ? t("remote.capabilityUnavailable") : undefined} onClick={onAddSource}>{t("sources.addToPool")}</Button></div></div>
      {availableCount ? <>
      {allAccounts.length ? <section>
        <header><strong>{t("connections.accounts")}</strong><span>{t("pool.availableAccounts", { count: allAccounts.length })}</span></header>
        <label className="relay-field"><span>{t("pool.searchAccounts")}</span><input type="search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("pool.searchAccountsPlaceholder")} /></label>
        {plans.length > 1 ? <div className="pool-member-plan-tools"><div className="account-plan-filters" role="group" aria-label={t("accounts.filterByPlan")}><span>{t("accounts.plan")}</span><button type="button" aria-pressed={activePlan === "all"} aria-label={t("accounts.planFilterOption", { plan: t("accounts.allPlans"), count: allAccounts.length })} onClick={() => setPlanFilter("all")}><span>{t("accounts.allPlans")}</span><small>{allAccounts.length}</small></button>{plans.map((plan) => <button key={plan.id} type="button" aria-pressed={activePlan === plan.id} aria-label={t("accounts.planFilterOption", { plan: plan.label, count: plan.count })} onClick={() => setPlanFilter(plan.id)}><span>{plan.label}</span><small>{plan.count}</small></button>)}</div><Button variant="secondary" icon={shownSelected ? <X aria-hidden /> : <CheckCheck aria-hidden />} disabled={!accounts.length} onClick={toggleShown}>{shownSelected ? t("pool.clearShown") : t("pool.selectShown", { count: accounts.length })}</Button></div> : null}
        <div className="pool-member-options">{accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} /><span className="pool-member-option-copy"><strong>{account.label}</strong></span><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></label>)}</div>
        {!accounts.length ? <p className="form-note">{t("pool.noMatchingAccounts")}</p> : null}
      </section> : null}
      {sources.length ? <section><header><strong>{t("connections.sources")}</strong></header><div className="pool-member-options">{sources.map((source) => <label key={source.id}><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={() => setSourceIds(toggle(sourceIds, source.id))} /><span className="pool-member-option-copy"><strong>{source.name}</strong><small>{source.baseUrl} · {t(`sources.roles.${apiSourceRole(source.priority)}`)}</small></span></label>)}</div></section> : null}
      </> : <EmptyState title={t("pool.noAvailableMembers")} description={t("pool.noAvailableMembersHint")} />}
    </div>
  </Dialog>;
}

function ModelsView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [priceModel, setPriceModel] = useState<ModelSummary | null>(null);
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  const models = runtime ? modelSummaries(runtime) : [];
  const modelGroups = groupModelSummariesForLauncher(models, runtime?.accounts ?? []);
  const canEditPrice = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("model_pricing"));
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  if (!models.length) return <EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} />;
  return <><section className="model-rules relay-compact-content">
    <header>
      <div className="model-rules-copy"><h2>{t("models.visible")}</h2><p>{t("models.explanation")}</p></div>
    </header>
    <div className="relay-table-wrap"><table className="relay-table model-rules-table">
      <colgroup><col data-column="model" /><col data-column="codex" /><col data-column="price" /><col data-column="members" /><col data-column="actions" /></colgroup>
      <thead><tr><th>{t("common.model")}</th><th>{t("models.codexColumn")}</th><th>{t("models.priceColumn")}</th><th>{t("pool.members")}</th><th>{t("common.actions")}</th></tr></thead>
      {modelGroups.map((group) => <tbody key={group.id}>
      <tr className="model-group-row"><th colSpan={5} scope="rowgroup"><strong>{t(`modelGroups.${group.id}`)}</strong><span>{t("models.groupCount", { count: group.items.length })}</span></th></tr>
      {group.items.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const hasPrice = model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
      const cachedInputPrice = model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion;
      const priceParts = hasPrice ? [
        { label: t("models.inputPriceLabel"), value: formatModelPrice(model.inputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.outputPriceLabel"), value: formatModelPrice(model.outputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.cachedInputPriceLabel"), value: formatModelPrice(cachedInputPrice!, i18n.language) },
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite5mMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite5mPriceLabel"), value: formatModelPrice(model.cacheWrite5mMicroUsdPerMillion, i18n.language) }] : []),
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite1hMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite1hPriceLabel"), value: formatModelPrice(model.cacheWrite1hMicroUsdPerMillion, i18n.language) }] : []),
      ] : [];
      const displayName = model.codexDisplayName || model.id;
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"}>
        <td data-column="model"><div className="model-rule-identity"><strong title={displayName}>{displayName}</strong>{displayName !== model.id ? <code title={model.id}>{model.id}</code> : null}<span className={`model-rule-state ${model.enabled ? "ready" : "disabled"}`}><StatusIcon status={model.enabled ? "ready" : "disabled"} label={t(model.enabled ? "models.available" : "models.disabled")} /><span>{t(model.enabled ? "models.available" : "models.disabled")}</span></span></div></td>
        <td data-column="codex"><div className={`model-codex-state ${model.codexVisible ? "visible" : "hidden"}`}><BrainCircuit aria-hidden /><span><strong>{t(model.codexVisible ? "models.codexVisible" : model.enabled ? "models.codexUnsupported" : "models.codexDisabled")}</strong></span></div></td>
        <td data-column="price"><div className="model-price">{hasPrice ? <>{priceParts.map((part) => <span className="model-price-value" key={part.label}><small>{part.label}</small><strong>{part.value}</strong></span>)}{model.customPrice ? <small className="model-price-note custom">{t("models.customPrice")}</small> : null}</> : <span className="model-price-empty muted">{t("models.priceUnavailable")}</span>}</div></td>
        <td data-column="members"><span className="model-members">{t("pool.membersCount", { count: model.memberCount })}</span></td>
        <td data-column="actions"><div className="model-rule-actions">{canEditPrice ? <IconButton data-model-price-edit={model.id} label={t("models.editPrice", { model: model.id })} icon={<Pencil aria-hidden />} onClick={() => setPriceModel(model)} /> : null}{model.reasoningConfigurable ? <IconButton data-model-reasoning-edit={model.id} label={t("models.editReasoning", { model: model.id })} icon={<BrainCircuit aria-hidden />} onClick={() => setReasoningModel(model)} /> : null}<IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
    })}</tbody>)}
    </table></div>
  </section>{priceModel ? <ModelPriceDialog key={priceModel.id} model={priceModel} onClose={() => setPriceModel(null)} /> : null}{reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}</>;
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [allowedLevels, setAllowedLevels] = useState(model.reasoningAllowedLevels ?? []);
  const operation = `model-reasoning-${model.id}`;
  const levels = model.reasoningLevels ?? [];
  const save = async () => {
    const ok = await perform(operation, () => mode === "local"
      ? relayCommands.setModelReasoning(model.id, allowedLevels)
      : relayCommands.remoteAction({ type: "set_model_reasoning" }, { modelId: model.id, allowedLevels }), "feedback.saved");
    if (ok) onClose();
  };
  const automatic = allowedLevels.length === 0;
  const toggleAllowedLevel = (level: string) => setAllowedLevels((current) => current.length ? toggle(current, level) : [level]);
  return <Dialog title={t("models.reasoningTitle")} onClose={onClose} footer={<><Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} onClick={() => void save()}>{t("common.save")}</Button></>}>
    <div className="model-reasoning-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.reasoningHint")}</span></div>
      <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningTitle")}>
        <button type="button" aria-pressed={automatic} className={automatic ? "selected" : undefined} onClick={() => setAllowedLevels([])}>{t("models.reasoningAuto")}</button>
        {levels.map((level) => <button key={level} type="button" role="checkbox" aria-checked={!automatic && allowedLevels.includes(level)} className={!automatic && allowedLevels.includes(level) ? "selected" : undefined} onClick={() => toggleAllowedLevel(level)}>{formatReasoningEffort(level)}</button>)}
      </div>
    </div>
  </Dialog>;
}

function ModelPriceDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [inputPrice, setInputPrice] = useState(formatEditableModelPrice(model.inputMicroUsdPerMillion));
  const [cachedInputPrice, setCachedInputPrice] = useState(formatEditableModelPrice(model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion));
  const [cacheWrite5mPrice, setCacheWrite5mPrice] = useState(formatEditableModelPrice(model.cacheWrite5mMicroUsdPerMillion ?? null));
  const [cacheWrite1hPrice, setCacheWrite1hPrice] = useState(formatEditableModelPrice(model.cacheWrite1hMicroUsdPerMillion ?? null));
  const [outputPrice, setOutputPrice] = useState(formatEditableModelPrice(model.outputMicroUsdPerMillion));
  const cacheWrite = supportsCacheWritePricing(model.id);
  const inputMicroUsd = parseEditableModelPrice(inputPrice);
  const cachedInputMicroUsd = parseEditableModelPrice(cachedInputPrice);
  const cacheWrite5mMicroUsd = parseOptionalEditableModelPrice(cacheWrite5mPrice);
  const cacheWrite1hMicroUsd = parseOptionalEditableModelPrice(cacheWrite1hPrice);
  const outputMicroUsd = parseEditableModelPrice(outputPrice);
  const operation = `model-price-${model.id}`;
  const valid = inputMicroUsd != null && cachedInputMicroUsd != null && outputMicroUsd != null && cacheWrite5mMicroUsd !== undefined && cacheWrite1hMicroUsd !== undefined;
  const setPrice = (input: number | null, cachedInput: number | null, write5m: number | null, write1h: number | null, output: number | null) => mode === "local"
    ? relayCommands.setModelPrice(model.id, input, cachedInput, write5m, write1h, output)
    : relayCommands.remoteAction({ type: "set_model_price" }, { modelId: model.id, inputMicroUsdPerMillion: input, cachedInputMicroUsdPerMillion: cachedInput, cacheWrite5mMicroUsdPerMillion: write5m, cacheWrite1hMicroUsdPerMillion: write1h, outputMicroUsdPerMillion: output });
  const save = async () => {
    if (!valid) return;
    const ok = await perform(operation, () => setPrice(inputMicroUsd, cachedInputMicroUsd, cacheWrite ? cacheWrite5mMicroUsd ?? null : null, cacheWrite ? cacheWrite1hMicroUsd ?? null : null, outputMicroUsd), "feedback.saved");
    if (ok) onClose();
  };
  const restore = async () => {
    const ok = await perform(operation, () => setPrice(null, null, null, null, null), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("models.priceTitle")} onClose={onClose} footer={<>{model.customPrice ? <Button className="model-price-restore" variant="secondary" icon={<RotateCcw aria-hidden />} disabled={busy === operation} onClick={() => void restore()}>{t("models.restorePrice")}</Button> : null}<Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} disabled={!valid} onClick={() => void save()}>{t("common.save")}</Button></>}>
    <div className="relay-form model-price-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.priceUnit")}</span></div>
      <section className="model-price-section">
        <h3>{t("models.tokenPrices")}</h3>
        <div className="model-price-grid">
          <ModelPriceField label={t("models.inputPriceLabel")} value={inputPrice} invalid={inputPrice.length > 0 && inputMicroUsd == null} onChange={setInputPrice} />
          <ModelPriceField label={t("models.outputPriceLabel")} value={outputPrice} invalid={outputPrice.length > 0 && outputMicroUsd == null} onChange={setOutputPrice} />
          <ModelPriceField label={t("models.cachedInputPriceLabel")} value={cachedInputPrice} invalid={cachedInputPrice.length > 0 && cachedInputMicroUsd == null} onChange={setCachedInputPrice} />
        </div>
      </section>
      {cacheWrite ? <section className="model-price-section">
        <h3>{t("models.cachePrices")}</h3>
        <div className="model-price-grid ttl">
          <ModelPriceField label={t("models.cacheWrite5mPriceLabel")} value={cacheWrite5mPrice} invalid={cacheWrite5mMicroUsd === undefined} onChange={setCacheWrite5mPrice} />
          <ModelPriceField label={t("models.cacheWrite1hPriceLabel")} value={cacheWrite1hPrice} invalid={cacheWrite1hMicroUsd === undefined} onChange={setCacheWrite1hPrice} />
        </div>
      </section> : null}
    </div>
  </Dialog>;
}

function ModelPriceField({ label, value, invalid, onChange }: { label: string; value: string; invalid: boolean; onChange: (value: string) => void }) {
  return <label className="model-price-field"><span className="model-price-label">{label}</span><span className="model-price-input"><span className="model-price-currency" aria-hidden>$</span><input aria-label={label} type="text" inputMode="decimal" autoComplete="off" spellCheck={false} value={value} placeholder="0.00" aria-invalid={invalid || undefined} onChange={(event) => onChange(event.target.value)} /></span></label>;
}

function formatApiEquivalent(microUsd: number, locale: string) { return `≈${new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(microUsd / 1_000_000)}`; }
function formatProviderMicroUsd(value: number, locale: string) { return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(value / 1_000_000); }
function formatModelPrice(microUsd: number, locale: string) { return `$${new Intl.NumberFormat(locale, { maximumFractionDigits: 6 }).format(microUsd / 1_000_000)}`; }
function formatReasoningEffort(effort: string) { return effort.replace(/[_-]+/g, " ").replace(/\b\w/g, (letter) => letter.toUpperCase()); }
type RoutingPolicyPayload = {
  maxRetryCandidates: number;
  cooldownAfterFailures: number;
  keepLastCandidateAvailable: boolean;
  routingStrategy: RoutingStrategy;
  defaultServiceTier: DefaultServiceTier;
  subscriptionPlanOrder: string[];
};

function persistRoutingPolicy(mode: RelayMode, payload: RoutingPolicyPayload) {
  return mode === "local"
    ? relayCommands.updateRouting(payload.routingStrategy, payload.maxRetryCandidates, payload.cooldownAfterFailures, payload.keepLastCandidateAvailable, payload.defaultServiceTier, payload.subscriptionPlanOrder)
    : relayCommands.remoteAction({ type: "set_routing_policy" }, payload);
}
