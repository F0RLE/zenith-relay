import { useEffect, useState, type DragEvent } from "react";
import { Activity, ArrowDown, ArrowRightLeft, ArrowUp, CalendarDays, CheckCheck, Clock3, Cloud, Download, Gauge, GripVertical, ListMinus, Loader2, Pencil, Play, Plus, Power, RefreshCw, Trash2, Upload, UserRound, X, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, ConfigurationPresetPreview, ModelSummary, RoutingStrategy, SourceSummary } from "../../api/types";
import { AccountPlanBadge, ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, PageHeader, QuotaStack, StatusIcon, Tabs, accountPlanOption, apiSourcePriority, apiSourceRole, compareAccountPlans, formatRemainingTime, isCodexOauthAccountEligible, operationalStatusTone, useConfirm } from "../../components/Ui";
import type { ApiSourceRole } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { SourceDialog } from "../connections/ConnectionsPage";

type View = "members" | "models";
type Member = (AccountSummary & { kind: "account" }) | (SourceSummary & { kind: "source" });
type SubscriptionPlanGroup = { id: string; label: string; count: number };

export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, activateCodexProfile, busy, perform, codexPoolOauthSelection } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [quotaPolicy, setQuotaPolicy] = useState(false);
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
  const poolReady = Boolean(runtime?.gateway.candidateCount && runtime.gateway.visibleModelIds.length);
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
    {canSaveConfigurationPreset ? <ActionMenu label={t("pool.configurationPreset")}><ActionMenuItem icon={<Download aria-hidden />} disabled={Boolean(busy)} onClick={() => void exportConfiguration()}>{t("pool.exportConfiguration")}</ActionMenuItem>{supportsConfigurationPresets ? <ActionMenuItem icon={<Upload aria-hidden />} disabled={Boolean(busy)} onClick={() => void previewConfiguration()}>{t("pool.importConfiguration")}</ActionMenuItem> : null}</ActionMenu> : null}
    {view === "members" ? <Button data-action="pool-add" variant="secondary" icon={<Plus aria-hidden />} aria-label={t("pool.addMember")} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : t("pool.addMember")} onClick={() => setAddMembers(true)}>{t("pool.addMemberShort")}</Button> : null}
    {mode === "local" ? <>
      <Button data-action="pool-toggle" variant="secondary" icon={running ? <Power aria-hidden /> : <Play aria-hidden />} aria-label={poolToggleLabel} busy={busy === "pool-toggle"} disabled={!running && !poolReady} title={!running && !poolReady ? t("pool.startUnavailable") : poolToggleLabel} onClick={() => void perform("pool-toggle", running ? relayCommands.stopGateway : relayCommands.startGateway, running ? "feedback.stopped" : "feedback.started")}>{poolToggleShortLabel}</Button>
      <Button data-action="pool-switch" variant="primary" icon={<ArrowRightLeft aria-hidden />} aria-label={t("pool.switchChatGPT")} busy={busy === "pool-switch"} disabled={!running || !poolReady} title={!poolReady ? t("pool.startUnavailable") : !running ? t("pool.start") : t("pool.switchChatGPT")} onClick={() => void switchCodexToPool()}>{t("pool.switchChatGPTShort")}</Button>
    </> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <MembersView onAdd={() => setAddMembers(true)} onQuotaPolicy={() => setQuotaPolicy(true)} onRoutingPolicy={() => setRoutingPolicy(true)} supportsRoutingSettings={supportsRoutingSettings} /> : null}{view === "models" ? <ModelsView /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{quotaPolicy ? <QuotaPolicyDialog onClose={() => setQuotaPolicy(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{configurationPreview ? <ConfigurationPresetDialog preview={configurationPreview} onClose={() => setConfigurationPreview(null)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
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

function MembersView({ onAdd, onQuotaPolicy, onRoutingPolicy, supportsRoutingSettings }: { onAdd: () => void; onQuotaPolicy: () => void; onRoutingPolicy: () => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy, codexPoolOauthSelection } = useRelayState();
  const confirm = useConfirm();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const serviceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const routingStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
  const subscriptionExpiryFormat = new Intl.DateTimeFormat(i18n.language, { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [nowMs, setNowMs] = useState(Date.now());
  const poolMembers: Member[] = [
    ...(runtime?.accounts ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "account" as const })),
    ...(runtime?.sources ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "source" as const })),
  ];
  const runtimeOrder = runtime?.gateway.routingOrder ?? [];
  const runtimeByMember = new Map(runtimeOrder.map((candidate) => [runtimeMemberKey(candidate), candidate]));
  const orderByMember = new Map(runtimeOrder.map((candidate, index) => [runtimeMemberKey(candidate), index]));
  const members = [...poolMembers].sort((left, right) => comparePoolMembers(left, right, orderByMember));
  const upcomingTimes = members.flatMap((member) => member.kind === "account"
    ? [member.subscription.activeUntilMs, member.quota.primary?.resetAtMs, member.quota.secondary?.resetAtMs, ...(member.quota.supplemental ?? []).map((item) => item.window.resetAtMs)].filter((value): value is number => value != null && value > nowMs)
    : []);
  const showSeconds = upcomingTimes.some((value) => value - nowMs < 60 * 60_000);
  useEffect(() => {
    const timer = window.setTimeout(() => setNowMs(Date.now()), showSeconds ? 1_000 : 60_000);
    return () => window.clearTimeout(timer);
  }, [nowMs, showSeconds]);
  const activeMembers = members.filter((member) => (runtimeByMember.get(memberKey(member))?.inFlight ?? 0) > 0);
  const lastUsedRuntime = runtimeOrder.reduce<CandidateRuntimeSnapshot | null>((latest, candidate) => candidate.lastUsedAtMs != null && (latest?.lastUsedAtMs == null || candidate.lastUsedAtMs > latest.lastUsedAtMs) ? candidate : latest, null);
  const lastUsedMember = lastUsedRuntime ? members.find((member) => memberKey(member) === runtimeMemberKey(lastUsedRuntime)) ?? null : null;
  const nextMember = members.find((member) => runtimeByMember.get(memberKey(member))?.available) ?? null;
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
  const refreshQuotas = () => perform("pool-quota-refresh", () => mode === "local"
    ? relayCommands.refreshAllAccountQuotas()
    : relayCommands.remoteAction({ type: "refresh_all_quotas" }), "feedback.refreshed");
  const updateServiceTier = (fast: boolean) => {
    const defaultServiceTier = fast ? "fast" : "standard";
    if (defaultServiceTier === serviceTier) return;
    const maxRetryCandidates = runtime?.gateway.maxRetryCandidates ?? 3;
    const subscriptionPlanOrder = runtime?.gateway.subscriptionPlanOrder ?? [];
    void perform("pool-service-tier", async () => mode === "local"
      ? relayCommands.updateRouting(routingStrategy, maxRetryCandidates, defaultServiceTier, subscriptionPlanOrder)
      : relayCommands.remoteAction({ type: "set_routing_policy" }, { maxRetryCandidates, routingStrategy, defaultServiceTier, subscriptionPlanOrder }));
  };
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={onAdd}>{t("pool.addMember")}</Button>} />;
  const statuses = members.map((member) => member.operationalStatus);
  const counts = {
    rotation: statuses.filter((status) => status === "rotation").length,
    quotaWait: statuses.filter((status) => status === "quotaWait").length,
    unavailable: statuses.filter((status) => status === "unavailable").length,
    disabled: statuses.filter((status) => status === "disabled").length,
  };
  return <>
    <div className="pool-controls">
      <div className="table-toolbar pool-member-toolbar">
        <div className="pool-priority-label" title={t("pool.priorityHint")}><Activity aria-hidden /><span><strong>{t("pool.priorityTitle")}</strong><small>{routingSummary}</small></span></div>
        <div className="inline-actions pool-quota-actions">
          <div className="pool-control-group" data-toolbar-group="routing">
            <label className="pool-speed-control" data-fast={serviceTier === "fast" ? "true" : "false"} title={t("pool.serviceTierHint")}>
              <Zap aria-hidden />
              <span className="pool-speed-copy"><small>{t("pool.serviceTier")}</small><strong>{t(`pool.serviceTiers.${serviceTier}`)}</strong></span>
              <input type="checkbox" role="switch" aria-label={t("pool.serviceTier")} checked={serviceTier === "fast"} disabled={busy === "pool-service-tier"} onChange={(event) => updateServiceTier(event.target.checked)} />
              <span className="pool-speed-track" aria-hidden><span /></span>
            </label>
            <IconButton label={t("pool.routingSettings")} icon={<Gauge aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={onRoutingPolicy} />
            <IconButton label={t("pool.refreshPolicy")} icon={<Clock3 aria-hidden />} disabled={!canRefreshQuota} onClick={onQuotaPolicy} />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            <Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "pool-quota-refresh"} disabled={!canRefreshQuota || !quotaAccountCount} title={!quotaAccountCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()}>{t("pool.refreshQuotas")}</Button>
          </div>
        </div>
      </div>
      <div className="pool-summary"><div><span>{t("pool.memberStatus.rotation")}</span><strong>{counts.rotation}</strong></div><div><span>{t("pool.memberStatus.quotaWait")}</span><strong>{counts.quotaWait}</strong></div><div><span>{t("pool.memberStatus.unavailable")}</span><strong>{counts.unavailable}</strong></div><div><span>{t("pool.memberStatus.disabled")}</span><strong>{counts.disabled}</strong></div></div>
    </div>
    <div className="pool-member-list" role="list" aria-label={t("pool.members")}>
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const runtimeState = runtimeByMember.get(memberId);
        const excludedByFreePolicy = member.kind === "account" && member.routingExclusion === "free_plan_policy";
        const statusKey = member.operationalStatus;
        const statusTone = operationalStatusTone(statusKey);
        const codexInterface = member.kind === "account" && codexPoolOauthSelection === member.id;
        const identity = member.kind === "source" ? member.name : member.identityHint || member.label;
        const detail = member.kind === "source"
          ? `${member.wireApi} · ${member.baseUrl}`
          : member.label;
        const subscriptionExpiry = member.kind === "account"
          ? member.subscription.activeUntilMs == null
            ? { date: t("pool.subscriptionExpiryUnknown"), remaining: null }
            : { date: subscriptionExpiryFormat.format(member.subscription.activeUntilMs), remaining: formatRemainingTime(member.subscription.activeUntilMs, nowMs, t) }
          : null;
        const isCurrent = (runtimeState?.inFlight ?? 0) > 0;
        const isLastUsed = !isCurrent && runtimeState != null && runtimeState.candidateId === lastUsedRuntime?.candidateId && runtimeState.kind === lastUsedRuntime.kind;
        const runtimeHint = runtimeState?.halfOpen
          ? t("pool.recoveryProbe")
          : member.kind === "source" && runtimeState?.nextRetryAtMs
            ? t("pool.retryAt", { time: new Date(runtimeState.nextRetryAtMs).toLocaleString(i18n.language) })
            : excludedByFreePolicy ? t("pool.freePolicyHint") : undefined;
        const editLabel = `${t("pool.editMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removeLabel = `${t("pool.removeMember")}: ${member.kind === "source" ? member.name : member.label}`;
        const removing = busy === `pool-remove-${member.id}`;
        const statusLabel = excludedByFreePolicy ? t("accounts.participation.freePolicy") : t(`pool.memberStatus.${statusKey}`);
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}${isCurrent ? " current" : ""}`} role="listitem" title={codexInterface ? t("pool.codexInterfaceHint") : undefined} data-member-label={member.kind === "source" ? member.name : member.label} data-current={isCurrent ? "true" : "false"} data-last-used={isLastUsed ? "true" : "false"} data-member-kind={member.kind}>
          <header className="pool-member-card-header">
            <div className="pool-member-kind-icon" aria-hidden>{member.kind === "source" ? <Cloud /> : <UserRound />}</div>
            <div className="pool-member-identity">
              <strong className="pool-member-name" title={identity === detail ? identity : `${identity} · ${detail}`}>{identity}</strong>
              <div className="pool-member-meta">{member.kind === "account" ? <AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} /> : <small title={detail}>{detail}</small>}<div className="pool-member-state"><StatusIcon status={statusTone} label={runtimeHint ? `${statusLabel} · ${runtimeHint}` : statusLabel} /></div></div>
            </div>
          </header>
          <div className="pool-member-card-quota">{member.kind === "account" ? <QuotaStack snapshot={member.quota} nowMs={nowMs} /> : <span className="pool-member-quota-unavailable">{t("quota.notReported")}</span>}</div>
          <div className="pool-member-context" data-kind={member.kind}>{member.kind === "account" ? <><CalendarDays aria-hidden /><span className="pool-member-subscription-date">{subscriptionExpiry?.date}</span>{subscriptionExpiry?.remaining ? <span className="pool-member-subscription-expiry">{subscriptionExpiry.remaining}</span> : null}</> : <><Cloud aria-hidden /><span>{t(`sources.roles.${apiSourceRole(member.priority)}`)}</span></>}</div>
          <footer className="pool-member-card-footer">
            <dl className="pool-member-routing"><div title={t("pool.apiEquivalentHint", { count: member.apiEquivalent.unpricedTokens })}><dt>{t("pool.apiEquivalent")}</dt><dd>{formatApiEquivalent(member.apiEquivalent.microUsd, i18n.language)}{member.apiEquivalent.unpricedTokens ? "*" : ""}</dd></div></dl>
            <div className="pool-member-actions">
              <IconButton label={editLabel} icon={<Pencil aria-hidden />} aria-haspopup="dialog" onClick={() => setSelectedId(memberId)} />
              <IconButton className="danger" label={removeLabel} icon={removing ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />} disabled={removing} onClick={() => void confirmRemove(member)} />
            </div>
          </footer>
        </article>;
      })}
    </div>
    {selected ? <MemberEditor key={`${selected.kind}:${selected.id}`} member={selected} onClose={() => setSelectedId(null)} /> : null}
  </>;
}

function QuotaPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [refreshIntervalSeconds, setRefreshIntervalSeconds] = useState(runtime?.gateway.quotaRefreshIntervalSeconds ?? 300);
  const requestTimeoutSeconds = runtime?.gateway.quotaRequestTimeoutSeconds ?? 20;
  const [useFreeAccounts, setUseFreeAccounts] = useState(runtime?.gateway.useFreeAccounts ?? false);
  const supportsFreeAccountPolicy = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("free_account_policy"));
  const save = async () => {
    const payload = { refreshIntervalSeconds, requestTimeoutSeconds, useFreeAccounts };
    const ok = await perform("quota-policy", () => mode === "local"
      ? relayCommands.updateQuotaPolicy(refreshIntervalSeconds, requestTimeoutSeconds, useFreeAccounts)
      : relayCommands.remoteAction({ type: "set_quota_policy" }, payload), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.refreshPolicyTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "quota-policy"} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form pool-policy-form">
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.refreshInterval")}</strong></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.refreshInterval")} value={String(refreshIntervalSeconds)} onChange={(value) => setRefreshIntervalSeconds(Number(value))} options={[{ value: "120", label: t("pool.refreshIntervals.twoMinutes") }, { value: "300", label: t("pool.refreshIntervals.fiveMinutes") }, { value: "600", label: t("pool.refreshIntervals.tenMinutes") }, { value: "1800", label: t("pool.refreshIntervals.thirtyMinutes") }, { value: "3600", label: t("pool.refreshIntervals.oneHour") }]} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.useFreeAccounts")}</strong><small>{t("pool.useFreeAccountsHint")}</small></div>
        <label className="toggle-row pool-policy-toggle"><input type="checkbox" aria-label={t("pool.useFreeAccounts")} checked={useFreeAccounts} disabled={!supportsFreeAccountPolicy} onChange={(event) => setUseFreeAccounts(event.target.checked)} /><span>{t(useFreeAccounts ? "common.enabled" : "common.disabled")}</span></label>
      </div>
      {!supportsFreeAccountPolicy ? <p className="form-note">{t("pool.freeAccountPolicyUnavailable")}</p> : null}
    </div>
  </Dialog>;
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
  const [hasCustomPlanOrder, setHasCustomPlanOrder] = useState(storedPlanOrder.length > 0 || initialStrategy === "subscription_plan");
  const [draggedPlan, setDraggedPlan] = useState<string | null>(null);
  const maxRetryCandidates = runtime?.gateway.maxRetryCandidates ?? 3;
  const movePlan = (plan: string, target: string, after = false) => {
    if (plan === target) return;
    setSubscriptionPlanOrder((current) => {
      const next = current.filter((value) => value !== plan);
      const targetIndex = next.indexOf(target);
      if (targetIndex < 0) return current;
      next.splice(targetIndex + (after ? 1 : 0), 0, plan);
      return next;
    });
    setHasCustomPlanOrder(true);
  };
  const movePlanBy = (plan: string, offset: number) => {
    const index = subscriptionPlanOrder.indexOf(plan);
    const target = subscriptionPlanOrder[index + offset];
    if (target) movePlan(plan, target, offset > 0);
  };
  const chooseStrategy = (value: string) => {
    const next = value as RoutingStrategy;
    setRoutingStrategy(next);
    if (next === "subscription_plan") setHasCustomPlanOrder(true);
  };
  const deletePlanOrder = () => {
    setSubscriptionPlanOrder(defaultPlanOrder);
    setHasCustomPlanOrder(false);
    setRoutingStrategy("adaptive");
  };
  const save = async () => {
    const savedPlanOrder = hasCustomPlanOrder ? subscriptionPlanOrder : [];
    const payload = {
      maxRetryCandidates,
      routingStrategy,
      defaultServiceTier,
      subscriptionPlanOrder: savedPlanOrder,
    };
    const ok = await perform("routing-policy", async () => {
      if (mode === "local") {
        return relayCommands.updateRouting(routingStrategy, maxRetryCandidates, defaultServiceTier, savedPlanOrder);
      }
      return relayCommands.remoteAction({ type: "set_routing_policy" }, payload);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "routing-policy"} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form pool-policy-form">
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.routingStrategy")}</strong><small>{t(`pool.routingStrategyHints.${routingStrategy}`)}</small></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.routingStrategy")} value={routingStrategy} onChange={chooseStrategy} options={[{ value: "adaptive", label: t("pool.routingStrategies.adaptive") }, { value: "quota_highest", label: t("pool.routingStrategies.quotaHighest") }, { value: "subscription_expiry", label: t("pool.routingStrategies.subscriptionExpiry") }, { value: "subscription_plan", label: t("pool.routingStrategies.subscriptionPlan") }]} />
      </div>
      {routingStrategy === "subscription_plan" ? <div className="subscription-plan-policy">
        <div className="subscription-plan-policy-heading"><div><strong>{t("pool.subscriptionPlanOrder")}</strong><small>{t("pool.subscriptionPlanOrderHint")}</small></div>{hasCustomPlanOrder ? <IconButton label={t("pool.deleteSubscriptionPlanOrder")} icon={<Trash2 aria-hidden />} onClick={deletePlanOrder} /> : null}</div>
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

function subscriptionPlanGroups(accounts: AccountSummary[], unknown: string): SubscriptionPlanGroup[] {
  const groups = new Map<string, SubscriptionPlanGroup>();
  for (const account of accounts.filter((account) => account.inPool)) {
    const id = account.subscription.planType?.trim().toLocaleLowerCase() || "unknown";
    const current = groups.get(id);
    if (current) current.count += 1;
    else groups.set(id, { id, label: accountPlanOption(account.subscription.planType, unknown).label, count: 1 });
  }
  return [...groups.values()].sort((left, right) => compareAccountPlans(accountPlanOption(left.id === "unknown" ? null : left.id, unknown), accountPlanOption(right.id === "unknown" ? null : right.id, unknown)));
}

function mergeSubscriptionPlanOrder(groups: SubscriptionPlanGroup[], saved: string[]) {
  const available = new Set(groups.map((group) => group.id));
  return [...saved.filter((plan) => available.delete(plan)), ...groups.map((group) => group.id).filter((plan) => available.has(plan))];
}

function MemberEditor({ member, onClose }: { member: Member; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [sourceRole, setSourceRole] = useState<ApiSourceRole>(apiSourceRole(member.priority));
  const modelIds = [...new Map([...member.models, ...member.allowedModels, ...member.excludedModels].map((model) => [model.toLocaleLowerCase(), model])).values()];
  const [enabledModels, setEnabledModels] = useState(() => {
    const allowed = new Set(member.allowedModels.map((model) => model.toLocaleLowerCase()));
    const excluded = new Set(member.excludedModels.map((model) => model.toLocaleLowerCase()));
    return modelIds.filter((model) => (!allowed.size || allowed.has(model.toLocaleLowerCase())) && !excluded.has(model.toLocaleLowerCase()));
  });
  const [draining, setDraining] = useState(member.draining);
  const save = async () => {
    const allEnabled = modelIds.every((model) => enabledModels.includes(model));
    const allowedModels = allEnabled ? [] : modelIds.filter((model) => enabledModels.includes(model));
    const excludedModels = allEnabled ? [] : modelIds.filter((model) => !enabledModels.includes(model));
    const ok = await perform(`member-${member.id}`, () => {
      if (member.kind === "account") {
        const payload = { allowedModels, excludedModels, draining };
        return mode === "local"
          ? relayCommands.updateAccount({ accountId: member.id, ...payload })
          : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      }
      const payload = { allowedModels, excludedModels, draining: member.draining, priority: apiSourcePriority(sourceRole), weight: member.weight };
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, ...payload };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide title={`${t("pool.editMember")} · ${member.kind === "source" ? member.name : member.label}`} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={save}>{t("pool.savePolicy")}</Button></>}>
    <div className="relay-form">
      {member.kind === "source" ? <section className="source-routing-section">
        <header className="source-routing-heading"><Gauge aria-hidden /><div><h3>{t("sources.poolRole")}</h3><small>{t("sources.routingHint")}</small></div></header>
        <div className="source-role-options" role="radiogroup" aria-label={t("sources.poolRole")}>
          {(["primary", "stabilizer", "reserve"] as ApiSourceRole[]).map((value) => <button key={value} type="button" role="radio" aria-checked={sourceRole === value} onClick={() => setSourceRole(value)}><strong>{t(`sources.roles.${value}`)}</strong><small>{t(`sources.roleHints.${value}`)}</small></button>)}
        </div>
      </section> : <div className="settings-row"><label className="toggle-row"><input type="checkbox" checked={draining} onChange={(event) => setDraining(event.target.checked)} /><span>{t("accounts.drain")}</span></label></div>}
      <section className="member-model-rules">
        <header><h2>{t("common.models")}</h2></header>
        {modelIds.length ? <ul>{modelIds.map((model) => {
          const enabled = enabledModels.includes(model);
          return <li key={model} data-member-model-id={model} data-enabled={enabled ? "true" : "false"}>
            <code>{model}</code>
            <StatusIcon status={enabled ? "ready" : "disabled"} label={t(enabled ? "models.available" : "models.disabled")} />
            <IconButton className="member-model-toggle" aria-pressed={enabled} label={t(enabled ? "models.disable" : "models.enable", { model })} icon={<Power aria-hidden />} onClick={() => setEnabledModels((values) => toggle(values, model))} />
          </li>;
        })}</ul> : <p className="form-note">{t("models.emptyDescription")}</p>}
      </section>
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
    .sort((left, right) => compareAccountPlans(accountPlanOption(left.subscription.planType, t("common.unknown")), accountPlanOption(right.subscription.planType, t("common.unknown"))) || left.label.localeCompare(right.label));
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
  const models = runtime ? modelSummaries(runtime).sort(compareModelCatalog) : [];
  const canEditPrice = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("model_pricing"));
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  if (!models.length) return <EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} />;
  return <><section className="model-rules">
    <header>
      <div className="model-rules-copy"><h2>{t("models.visible")}</h2><p>{t("models.explanation")}</p></div>
    </header>
    <div className="relay-table-wrap"><table className="relay-table model-rules-table">
      <colgroup><col data-column="model" /><col data-column="status" /><col data-column="price" /><col data-column="members" /><col data-column="actions" /></colgroup>
      <thead><tr><th>{t("common.model")}</th><th>{t("common.status")}</th><th>{t("models.priceColumn")}</th><th>{t("pool.members")}</th><th>{t("common.actions")}</th></tr></thead>
      <tbody>{models.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const hasPrice = model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
      const cachedInputPrice = model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion;
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"}>
        <td data-column="model"><code>{model.id}</code></td>
        <td data-column="status"><StatusIcon status={model.enabled ? "ready" : "disabled"} label={t(model.enabled ? "models.available" : "models.disabled")} /></td>
        <td data-column="price"><div className="model-price">{hasPrice ? <><span>{t("models.inputPrice", { price: formatModelPrice(model.inputMicroUsdPerMillion!, i18n.language) })}</span><span>{t("models.outputPrice", { price: formatModelPrice(model.outputMicroUsdPerMillion!, i18n.language) })}</span><span>{t("models.cachedInputPrice", { price: formatModelPrice(cachedInputPrice!, i18n.language) })}</span><small className={model.customPrice ? "custom" : undefined}>{t(model.customPrice ? "models.customPrice" : "models.perMillion")}</small></> : <span className="muted">{t("models.priceUnavailable")}</span>}</div></td>
        <td data-column="members"><span className="model-members">{t("pool.membersCount", { count: model.memberCount })}</span></td>
        <td data-column="actions"><div className="model-rule-actions">{canEditPrice ? <IconButton data-model-price-edit={model.id} label={t("models.editPrice", { model: model.id })} icon={<Pencil aria-hidden />} onClick={() => setPriceModel(model)} /> : null}<IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
    })}</tbody>
    </table></div>
  </section>{priceModel ? <ModelPriceDialog key={priceModel.id} model={priceModel} onClose={() => setPriceModel(null)} /> : null}</>;
}

function ModelPriceDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [inputPrice, setInputPrice] = useState(formatEditableModelPrice(model.inputMicroUsdPerMillion));
  const [cachedInputPrice, setCachedInputPrice] = useState(formatEditableModelPrice(model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion));
  const [outputPrice, setOutputPrice] = useState(formatEditableModelPrice(model.outputMicroUsdPerMillion));
  const inputMicroUsd = parseEditableModelPrice(inputPrice);
  const cachedInputMicroUsd = parseEditableModelPrice(cachedInputPrice);
  const outputMicroUsd = parseEditableModelPrice(outputPrice);
  const operation = `model-price-${model.id}`;
  const valid = inputMicroUsd != null && cachedInputMicroUsd != null && outputMicroUsd != null;
  const setPrice = (input: number | null, cachedInput: number | null, output: number | null) => mode === "local"
    ? relayCommands.setModelPrice(model.id, input, cachedInput, output)
    : relayCommands.remoteAction({ type: "set_model_price" }, { modelId: model.id, inputMicroUsdPerMillion: input, cachedInputMicroUsdPerMillion: cachedInput, outputMicroUsdPerMillion: output });
  const save = async () => {
    if (!valid) return;
    const ok = await perform(operation, () => setPrice(inputMicroUsd, cachedInputMicroUsd, outputMicroUsd), "feedback.saved");
    if (ok) onClose();
  };
  const restore = async () => {
    const ok = await perform(operation, () => setPrice(null, null, null), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("models.priceTitle", { model: model.id })} onClose={onClose} footer={<>{model.customPrice ? <Button variant="secondary" disabled={busy === operation} onClick={() => void restore()}>{t("models.restorePrice")}</Button> : null}<Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} disabled={!valid} onClick={() => void save()}>{t("common.save")}</Button></>}>
    <div className="relay-form model-price-form">
      <p>{t("models.priceHint")}</p>
      <div className="model-price-fields">
        <label className="relay-field"><span>{t("models.inputPriceLabel")}</span><div className="model-price-input"><span aria-hidden>$</span><input aria-label={t("models.inputPriceLabel")} type="number" inputMode="decimal" min="0" max="1000000" step="0.000001" value={inputPrice} aria-invalid={inputPrice.length > 0 && inputMicroUsd == null} onChange={(event) => setInputPrice(event.target.value)} /></div><small>{t("models.priceUnit")}</small></label>
        <label className="relay-field"><span>{t("models.outputPriceLabel")}</span><div className="model-price-input"><span aria-hidden>$</span><input aria-label={t("models.outputPriceLabel")} type="number" inputMode="decimal" min="0" max="1000000" step="0.000001" value={outputPrice} aria-invalid={outputPrice.length > 0 && outputMicroUsd == null} onChange={(event) => setOutputPrice(event.target.value)} /></div><small>{t("models.priceUnit")}</small></label>
        <label className="relay-field"><span>{t("models.cachedInputPriceLabel")}</span><div className="model-price-input"><span aria-hidden>$</span><input aria-label={t("models.cachedInputPriceLabel")} type="number" inputMode="decimal" min="0" max="1000000" step="0.000001" value={cachedInputPrice} aria-invalid={cachedInputPrice.length > 0 && cachedInputMicroUsd == null} onChange={(event) => setCachedInputPrice(event.target.value)} /></div><small>{t("models.priceUnit")}</small></label>
      </div>
      <p className="form-note">{t("models.priceCalculationHint")}</p>
    </div>
  </Dialog>;
}

function toggle(values: string[], value: string) { return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]; }
function formatApiEquivalent(microUsd: number, locale: string) { return `≈${new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(microUsd / 1_000_000)}`; }
function formatModelPrice(microUsd: number, locale: string) { return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(microUsd / 1_000_000); }
function formatEditableModelPrice(microUsd: number | null) { return microUsd == null ? "" : (microUsd / 1_000_000).toFixed(6).replace(/\.?0+$/, ""); }
function parseEditableModelPrice(value: string) {
  const normalized = value.trim();
  if (!/^\d+(?:\.\d{0,6})?$/.test(normalized)) return null;
  const price = Number(normalized);
  return Number.isFinite(price) && price >= 0 && price <= 1_000_000 ? Math.round(price * 1_000_000) : null;
}
function modelSummaries(runtime: NonNullable<ReturnType<typeof useRelayState>["runtime"]>): ModelSummary[] {
  if (runtime.gateway.models?.length) return [...runtime.gateway.models];
  return runtime.gateway.visibleModelIds.map((id) => ({
    id,
    enabled: true,
    memberCount: [...runtime.sources, ...runtime.accounts].filter((member) => member.models.some((model) => model.toLowerCase() === id.toLowerCase())).length,
    catalogRank: null,
    inputMicroUsdPerMillion: null,
    cachedInputMicroUsdPerMillion: null,
    outputMicroUsdPerMillion: null,
    customPrice: false,
  }));
}
function compareModelCatalog(left: ModelSummary, right: ModelSummary) {
  return (left.catalogRank ?? Number.MAX_SAFE_INTEGER) - (right.catalogRank ?? Number.MAX_SAFE_INTEGER)
    || compareModelPrice(left, right, -1)
    || left.id.localeCompare(right.id);
}
function compareModelPrice(left: ModelSummary, right: ModelSummary, direction: 1 | -1) {
  const leftKnown = left.inputMicroUsdPerMillion != null && left.outputMicroUsdPerMillion != null;
  const rightKnown = right.inputMicroUsdPerMillion != null && right.outputMicroUsdPerMillion != null;
  if (leftKnown !== rightKnown) return leftKnown ? -1 : 1;
  if (!leftKnown) return 0;
  return direction * (left.outputMicroUsdPerMillion! - right.outputMicroUsdPerMillion!)
    || direction * (left.inputMicroUsdPerMillion! - right.inputMicroUsdPerMillion!);
}
function comparePoolMembers(left: Member, right: Member, order: Map<string, number>) {
  return (order.get(memberKey(left)) ?? Number.MAX_SAFE_INTEGER) - (order.get(memberKey(right)) ?? Number.MAX_SAFE_INTEGER)
    || memberName(left).localeCompare(memberName(right));
}
function memberKey(member: Member) { return `${member.kind}:${member.id}`; }
function runtimeMemberKey(candidate: CandidateRuntimeSnapshot) { return `${candidate.kind === "api_source" ? "source" : "account"}:${candidate.candidateId}`; }
function memberName(member: Member) { return member.kind === "source" ? member.name : member.identityHint || member.label; }
