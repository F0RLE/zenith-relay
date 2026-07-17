import { useEffect, useMemo, useState } from "react";
import { Activity, ArrowRightLeft, CheckCheck, Clock3, Gauge, KeyRound, Loader2, Pencil, Play, Plus, Power, RefreshCw, RotateCcw, Trash2, X, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot, KeySummary, ModelSummary, RoutingStrategy, SourceSummary } from "../../api/types";
import { AccountPlanBadge, ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, PageHeader, QuotaStack, StatusBadge, Tabs, accountPlanOption, apiSourcePriority, apiSourceRole, compareAccountPlans, isCodexOauthAccountEligible, useConfirm } from "../../components/Ui";
import type { ApiSourceRole } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { SourceDialog } from "../connections/ConnectionsPage";

type View = "members" | "keys" | "models";
type Member = (AccountSummary & { kind: "account" }) | (SourceSummary & { kind: "source"; health: string; quota: null });

export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, activateCodexProfile, busy, perform, codexPoolOauthSelection } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createKey, setCreateKey] = useState(false);
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [quotaPolicy, setQuotaPolicy] = useState(false);
  const [routingPolicy, setRoutingPolicy] = useState(false);
  const supportsKeys = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("keys"));
  const supportsModels = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("models"));
  const supportsMembers = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const supportsRoutingSettings = mode !== "remote" || runtime?.gateway.maxRetryCandidates != null;
  useEffect(() => {
    if ((view === "keys" && !supportsKeys) || (view === "models" && !supportsModels)) setView("members");
  }, [view, supportsKeys, supportsModels]);
  const viewAction = view === "keys"
    ? <Button variant="secondary" icon={<KeyRound aria-hidden />} disabled={!supportsKeys} title={!supportsKeys ? t("remote.capabilityUnavailable") : undefined} onClick={() => setCreateKey(true)}>{t("keys.create")}</Button>
    : view === "members"
      ? <Button variant="secondary" icon={<Plus aria-hidden />} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : undefined} onClick={() => setAddMembers(true)}>{t("pool.addMember")}</Button>
      : null;
  const poolReady = Boolean(runtime?.gateway.candidateCount && runtime.gateway.visibleModelIds.length);
  const selectedOauthAccountId = codexPoolOauthSelection !== "none" && codexPoolOauthSelection !== "auto"
    && runtime?.accounts.some((account) => account.id === codexPoolOauthSelection && isCodexOauthAccountEligible(account))
    ? codexPoolOauthSelection
    : null;
  const switchCodexToPool = () => activateCodexProfile("pool-switch", async () => {
    const snapshot = await relayCommands.localState();
    const key = snapshot.keys.find((candidate) => candidate.system && candidate.enabled)
      ?? (await relayCommands.createKey(t("pool.codexKeyLabel"), true)).key;
    return relayCommands.attachCodexGateway(key.id, selectedOauthAccountId, codexPoolOauthSelection === "none");
  }, true);
  const running = Boolean(runtime?.gateway.running);
  const poolToggleLabel = running ? t("pool.stop") : t("pool.start");
  const action = <div className="pool-header-actions">
    {viewAction}
    {mode === "local" ? <>
      <Button data-action="pool-toggle" variant="secondary" icon={running ? <Power aria-hidden /> : <Play aria-hidden />} aria-label={poolToggleLabel} busy={busy === "pool-toggle"} disabled={!running && !poolReady} title={!running && !poolReady ? t("pool.startUnavailable") : poolToggleLabel} onClick={() => void perform("pool-toggle", running ? relayCommands.stopGateway : relayCommands.startGateway, running ? "feedback.stopped" : "feedback.started")}>{poolToggleLabel}</Button>
      <Button variant="primary" icon={<ArrowRightLeft aria-hidden />} busy={busy === "pool-switch"} disabled={!running || !poolReady} title={!poolReady ? t("pool.startUnavailable") : !running ? t("pool.start") : undefined} onClick={() => void switchCodexToPool()}>{t("pool.switchChatGPT")}</Button>
    </> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsKeys ? [{ id: "keys", label: t("pool.keys") }] : []), ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <MembersView onAdd={() => setAddMembers(true)} onQuotaPolicy={() => setQuotaPolicy(true)} onRoutingPolicy={() => setRoutingPolicy(true)} supportsRoutingSettings={supportsRoutingSettings} /> : null}{view === "keys" ? <KeysView onCreate={() => setCreateKey(true)} /> : null}{view === "models" ? <ModelsView /> : null}{createKey ? <CreateKeyDialog onClose={() => setCreateKey(false)} /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{quotaPolicy ? <QuotaPolicyDialog onClose={() => setQuotaPolicy(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function MembersView({ onAdd, onQuotaPolicy, onRoutingPolicy, supportsRoutingSettings }: { onAdd: () => void; onQuotaPolicy: () => void; onRoutingPolicy: () => void; supportsRoutingSettings: boolean }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy, codexPoolOauthSelection } = useRelayState();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const supportsServiceTier = mode !== "remote" || runtime?.gateway.defaultServiceTier != null;
  const serviceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const poolMembers: Member[] = [
    ...(runtime?.accounts ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "account" as const })),
    ...(runtime?.sources ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "source" as const, health: item.enabled ? "healthy" : "disabled", quota: null })),
  ];
  const runtimeOrder = runtime?.gateway.routingOrder ?? [];
  const runtimeByMember = new Map(runtimeOrder.map((candidate) => [runtimeMemberKey(candidate), candidate]));
  const orderByMember = new Map(runtimeOrder.map((candidate, index) => [runtimeMemberKey(candidate), index]));
  const members = [...poolMembers].sort((left, right) => comparePoolMembers(left, right, orderByMember));
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
  const quotaAccountCount = members.filter((member) => member.kind === "account" && member.enabled).length;
  const refreshQuotas = () => perform("pool-quota-refresh", () => mode === "local"
    ? relayCommands.refreshPoolAccountQuotas()
    : relayCommands.remoteAction({ type: "refresh_pool_quotas" }), "feedback.refreshed");
  const updateServiceTier = (fast: boolean) => {
    const defaultServiceTier = fast ? "fast" : "standard";
    if (defaultServiceTier === serviceTier) return;
    const maxRetryCandidates = runtime?.gateway.maxRetryCandidates ?? 3;
    const routingStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
    const imageBaseModel = runtime?.gateway.imageBaseModel;
    void perform("pool-service-tier", async () => {
      if (mode === "local") return relayCommands.updateRouting(routingStrategy, maxRetryCandidates, defaultServiceTier, imageBaseModel ?? null);
      return relayCommands.remoteAction({ type: "set_routing_policy" }, { maxRetryCandidates, routingStrategy, defaultServiceTier, ...(imageBaseModel !== undefined ? { imageBaseModel } : {}) });
    });
  };
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={onAdd}>{t("pool.addMember")}</Button>} />;
  const statuses = members.map((member) => poolMemberStatus(member, runtimeByMember.get(memberKey(member))));
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
            <label className="pool-speed-control" data-fast={serviceTier === "fast" ? "true" : "false"} title={supportsServiceTier ? t("pool.serviceTierHint") : t("remote.capabilityUnavailable")}>
              <Zap aria-hidden />
              <span className="pool-speed-copy"><small>{t("pool.serviceTier")}</small><strong>{t(`pool.serviceTiers.${serviceTier}`)}</strong></span>
              <input type="checkbox" role="switch" aria-label={t("pool.serviceTier")} checked={serviceTier === "fast"} disabled={!supportsServiceTier || busy === "pool-service-tier"} onChange={(event) => updateServiceTier(event.target.checked)} />
              <span className="pool-speed-track" aria-hidden><span /></span>
            </label>
            <IconButton className="pool-routing-settings-button" label={t("pool.routingSettings")} icon={<Gauge aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={onRoutingPolicy} />
            <IconButton label={t("pool.refreshPolicy")} icon={<Clock3 aria-hidden />} disabled={!canRefreshQuota} onClick={onQuotaPolicy} />
          </div>
          <div className="pool-control-group" data-toolbar-group="refresh">
            <Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "pool-quota-refresh"} disabled={!canRefreshQuota || !quotaAccountCount} title={!quotaAccountCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()}>{t("pool.refreshQuotas")}</Button>
          </div>
        </div>
      </div>
      <div className="pool-summary"><div><span>{t("pool.memberStatus.rotation")}</span><strong>{counts.rotation}</strong></div><div><span>{t("pool.memberStatus.quotaWait")}</span><strong>{counts.quotaWait}</strong></div><div><span>{t("pool.memberStatus.unavailable")}</span><strong>{counts.unavailable}</strong></div><div><span>{t("pool.memberStatus.disabled")}</span><strong>{counts.disabled}</strong></div></div>
    </div>
    <div className="pool-member-list" role="list" aria-label={t("pool.members")} data-layout="list">
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const runtimeState = runtimeByMember.get(memberId);
        const excludedByFreePolicy = member.kind === "account" && member.routingExclusion === "free_plan_policy";
        const statusKey = poolMemberStatus(member, runtimeState);
        const statusTone = statusKey === "rotation" ? "ready" : statusKey === "disabled" ? "disabled" : statusKey === "quotaWait" ? "warning" : "error";
        const codexInterface = member.kind === "account" && codexPoolOauthSelection === member.id;
        const identity = member.kind === "source" ? member.name : member.identityHint || member.label;
        const detail = member.kind === "source"
          ? `${member.wireApi} · ${member.baseUrl} · ${t(`sources.roles.${apiSourceRole(member.priority)}`)}`
          : member.label;
        const quota = memberQuota(member);
        const isCurrent = (runtimeState?.inFlight ?? 0) > 0;
        const isLastUsed = !isCurrent && runtimeState != null && runtimeState.candidateId === lastUsedRuntime?.candidateId && runtimeState.kind === lastUsedRuntime.kind;
        const runtimeHint = runtimeState?.halfOpen
          ? t("pool.recoveryProbe")
          : runtimeState?.nextRetryAtMs
            ? t("pool.retryAt", { time: new Date(runtimeState.nextRetryAtMs).toLocaleTimeString(i18n.language) })
            : excludedByFreePolicy ? t("pool.freePolicyHint") : undefined;
        const editLabel = `${t("pool.editMember")}: ${member.kind === "source" ? member.name : member.label}`;
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}${isCurrent ? " current" : ""}`} role="listitem" data-member-label={member.kind === "source" ? member.name : member.label} data-current={isCurrent ? "true" : "false"} data-last-used={isLastUsed ? "true" : "false"}>
          <div className="pool-member-card-main">
            <div className="pool-member-state" title={runtimeHint}><StatusBadge status={statusTone} label={t(`pool.memberStatus.${statusKey}`)} />{isCurrent ? <small className="pool-member-current"><Activity aria-hidden />{runtimeState?.halfOpen ? t("pool.recoveryProbe") : runtimeState && runtimeState.inFlight > 1 ? t("pool.activeRequests", { count: runtimeState.inFlight }) : t("pool.currentRoute")}</small> : isLastUsed ? <small className="pool-member-last"><Clock3 aria-hidden />{t("pool.lastRoute")}</small> : <small title={codexInterface ? t("pool.codexInterfaceHint") : undefined}>{t(`pool.types.${member.kind}`)}{codexInterface ? ` · ${t("pool.codexInterface")}` : ""}</small>}</div>
            <div className="pool-member-identity"><strong title={identity}>{identity}</strong>{member.kind === "source" ? <small title={detail}>{detail}</small> : <div className="pool-member-account-meta">{identity !== member.label ? <small title={detail}>{detail}</small> : null}<AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} /></div>}</div>
            <div className="pool-member-quota-summary" title={quota == null ? t("common.unsupported") : t("pool.quotaRemaining")}><span>{t("pool.quotaRemaining")}</span><strong>{quota == null ? "-" : `${Math.round(quota / 100)}%`}</strong></div>
            <dl className="pool-member-routing"><div title={t("pool.apiEquivalentHint", { count: member.apiEquivalent.unpricedTokens })}><dt>{t("pool.apiEquivalent")}</dt><dd>{formatApiEquivalent(member.apiEquivalent.microUsd, i18n.language)}{member.apiEquivalent.unpricedTokens ? "*" : ""}</dd></div></dl>
            <IconButton label={editLabel} icon={<Pencil aria-hidden />} aria-haspopup="dialog" onClick={() => setSelectedId(memberId)} />
          </div>
          {member.kind === "account" ? <div className="account-card-quota pool-member-quota"><QuotaStack snapshot={member.quota} /></div> : null}
        </article>;
      })}
    </div>
    {selected ? <MemberEditor key={`${selected.kind}:${selected.id}`} member={selected} onClose={() => setSelectedId(null)} onRemove={() => void remove(selected)} /> : null}
  </>;
}

function QuotaPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [refreshIntervalSeconds, setRefreshIntervalSeconds] = useState(runtime?.gateway.quotaRefreshIntervalSeconds ?? 300);
  const requestTimeoutSeconds = runtime?.gateway.quotaRequestTimeoutSeconds ?? 20;
  const [useFreeAccounts, setUseFreeAccounts] = useState(runtime?.gateway.useFreeAccounts ?? mode === "remote");
  const supportsFreePolicy = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("free_account_policy"));
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
        <div className="pool-policy-copy"><strong>{t("pool.useFreeAccounts")}</strong><small>{supportsFreePolicy ? t("pool.useFreeAccountsHint") : t("pool.useFreeAccountsLegacyHint")}</small></div>
        <label className="toggle-row pool-policy-toggle"><input type="checkbox" aria-label={t("pool.useFreeAccounts")} checked={useFreeAccounts} disabled={!supportsFreePolicy} title={!supportsFreePolicy ? t("remote.capabilityUnavailable") : undefined} onChange={(event) => setUseFreeAccounts(event.target.checked)} /><span>{t(useFreeAccounts ? "common.enabled" : "common.disabled")}</span></label>
      </div>
    </div>
  </Dialog>;
}

function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const supportsRoutingStrategy = mode !== "remote" || runtime?.gateway.routingStrategy != null;
  const supportsImageBaseModel = mode !== "remote" || runtime?.gateway.imageBaseModel !== undefined;
  const [routingStrategy, setRoutingStrategy] = useState<RoutingStrategy>(runtime?.gateway.routingStrategy ?? "adaptive");
  const defaultServiceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const [imageBaseModel, setImageBaseModel] = useState(runtime?.gateway.imageBaseModel ?? "auto");
  const maxRetryCandidates = runtime?.gateway.maxRetryCandidates ?? 3;
  const imageModelOptions = useMemo(() => {
    const ids = new Map<string, string>();
    const models = [...(runtime?.gateway.models ?? [])]
      .filter((model) => model.enabled && !model.id.toLowerCase().includes("image"))
      .sort((left, right) => compareModelPrice(left, right, 1) || compareModelCatalog(left, right));
    for (const model of models) {
      ids.set(model.id.toLowerCase(), model.id);
    }
    if (imageBaseModel !== "auto" && imageBaseModel.trim()) ids.set(imageBaseModel.toLowerCase(), imageBaseModel);
    return [
      { value: "auto", label: t("pool.imageBaseModels.auto") },
      ...[...ids.values()].map((model) => ({ value: model, label: model })),
    ];
  }, [imageBaseModel, runtime?.gateway.models, t]);
  const save = async () => {
    const payload = {
      maxRetryCandidates,
      ...(supportsRoutingStrategy ? { routingStrategy } : {}),
      ...(supportsImageBaseModel ? { imageBaseModel: imageBaseModel === "auto" ? null : imageBaseModel } : {}),
    };
    const ok = await perform("routing-policy", async () => {
      if (mode === "local") {
        return relayCommands.updateRouting(routingStrategy, maxRetryCandidates, defaultServiceTier, imageBaseModel === "auto" ? null : imageBaseModel);
      }
      return relayCommands.remoteAction({ type: "set_routing_policy" }, payload);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "routing-policy"} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form pool-policy-form">
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.routingStrategy")}</strong><small>{supportsRoutingStrategy ? t(`pool.routingStrategyHints.${routingStrategy}`) : t("remote.capabilityUnavailable")}</small></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.routingStrategy")} value={routingStrategy} disabled={!supportsRoutingStrategy} onChange={(value) => setRoutingStrategy(value as RoutingStrategy)} options={[{ value: "adaptive", label: t("pool.routingStrategies.adaptive") }, { value: "oldest_account", label: t("pool.routingStrategies.oldestAccount") }]} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.imageBaseModel")}</strong><small>{supportsImageBaseModel ? t("pool.imageBaseModelHint") : t("remote.capabilityUnavailable")}</small></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.imageBaseModel")} value={imageBaseModel} disabled={!supportsImageBaseModel} onChange={setImageBaseModel} options={imageModelOptions} />
      </div>
    </div>
  </Dialog>;
}

function MemberEditor({ member, onClose, onRemove }: { member: Member; onClose: () => void; onRemove: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [sourceRole, setSourceRole] = useState<ApiSourceRole>(apiSourceRole(member.priority));
  const [weight, setWeight] = useState(member.weight);
  const [allowed, setAllowed] = useState(member.allowedModels.join(", "));
  const [excluded, setExcluded] = useState(member.excludedModels.join(", "));
  const [draining, setDraining] = useState(member.draining);
  const save = async () => {
    const allowedModels = parseList(allowed);
    const excludedModels = parseList(excluded);
    const ok = await perform(`member-${member.id}`, () => {
      if (member.kind === "account") {
        const payload = { allowedModels, excludedModels, draining };
        return mode === "local"
          ? relayCommands.updateAccount({ accountId: member.id, ...payload })
          : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      }
      const payload = { allowedModels, excludedModels, draining, priority: apiSourcePriority(sourceRole), weight };
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, ...payload };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide title={`${t("pool.editMember")} · ${member.kind === "source" ? member.name : member.label}`} onClose={onClose} footer={<><Button variant="danger" onClick={onRemove}>{t("pool.removeMember")}</Button><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={save}>{t("pool.savePolicy")}</Button></>}><div className="relay-form"><div className="settings-row">{member.kind === "source" ? <><div className="relay-field"><span>{t("sources.poolRole")}</span><OptionMenu className="field-option-menu" label={t("sources.poolRole")} value={sourceRole} onChange={(value) => setSourceRole(value as ApiSourceRole)} options={[{ value: "primary", label: t("sources.roles.primary") }, { value: "stabilizer", label: t("sources.roles.stabilizer") }, { value: "reserve", label: t("sources.roles.reserve") }]} /><small>{t(`sources.roleHints.${sourceRole}`)}</small></div><label><span title={t("pool.weightHelp")}>{t("pool.trafficShare")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label></> : null}<label className="toggle-row"><input type="checkbox" checked={draining} onChange={(event) => setDraining(event.target.checked)} /><span>{t("accounts.drain")}</span></label></div><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} placeholder={t("sources.modelListPlaceholder")} /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><p className="form-note">{t("pool.modelListHint")}</p></div></Dialog>;
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
        <div className="pool-member-options">{accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} /><span><strong>{account.identityHint || account.label}</strong><small>{account.label}</small></span><AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} /></label>)}</div>
        {!accounts.length ? <p className="form-note">{t("pool.noMatchingAccounts")}</p> : null}
      </section> : null}
      {sources.length ? <section><header><strong>{t("connections.sources")}</strong></header><div className="pool-member-options">{sources.map((source) => <label key={source.id}><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={() => setSourceIds(toggle(sourceIds, source.id))} /><span><strong>{source.name}</strong><small>{source.baseUrl} · {t(`sources.roles.${apiSourceRole(source.priority)}`)}</small></span></label>)}</div></section> : null}
      </> : <EmptyState title={t("pool.noAvailableMembers")} description={t("pool.noAvailableMembersHint")} />}
    </div>
  </Dialog>;
}

function KeysView({ onCreate }: { onCreate: () => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("keys"));
  const [revealed, setRevealed] = useState("");
  const [editing, setEditing] = useState<KeySummary | null>(null);
  const keys = (runtime?.keys ?? []).filter((key) => !key.system);
  if (!keys.length) return <EmptyState title={t("keys.emptyTitle")} description={t("keys.emptyDescription")} action={<Button variant="primary" disabled={!canManage} title={!canManage ? t("remote.capabilityUnavailable") : undefined} onClick={onCreate}>{t("keys.create")}</Button>} />;
  const formatLastUsed = new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" });
  return <>
    <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("keys.masked")}</th><th>{t("keys.scope")}</th><th>{t("common.models")}</th><th>{t("common.lastUsed")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead><tbody>{keys.map((key) => <tr key={key.id}>
      <td><StatusBadge status={key.enabled ? "ready" : "disabled"} label={key.enabled ? t("common.enabled") : t("common.disabled")} /></td>
      <td>{key.label}</td>
      <td><code>zlr_••••••••••••</code></td>
      <td>{(key.accountIds?.length || key.sourceIds?.length) ? t("keys.scoped") : t("keys.allMembers")}</td>
      <td>{key.allowedModels.length || t("keys.allModels")}</td>
      <td>{key.lastUsedAtMs ? formatLastUsed.format(new Date(key.lastUsedAtMs)) : t("common.never")}</td>
      <td className="row-actions"><IconButton label={t("keys.editPolicy")} icon={<Pencil aria-hidden />} onClick={() => setEditing(key)} /><ActionMenu>
        <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`enable-${key.id}`, () => mode === "local" ? relayCommands.setKeyEnabled(key.id, !key.enabled) : relayCommands.remoteAction({ type: "update_key", id: key.id }, { enabled: !key.enabled }), "feedback.saved")}>{key.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
        <ActionMenuItem icon={<RotateCcw aria-hidden />} disabled={busy === `rotate-${key.id}`} onClick={async () => {
          if (!await confirm(t("keys.rotateConfirm"))) return;
          const result: { current: { secret: string } | null } = { current: null };
          await perform(`rotate-${key.id}`, async () => { result.current = mode === "local" ? await relayCommands.rotateKey(key.id) : await relayCommands.remoteAction({ type: "rotate_key", id: key.id }) as { secret: string }; }, "feedback.keyRotated");
          if (result.current) setRevealed(result.current.secret);
        }}>{t("keys.rotate")}</ActionMenuItem>
        <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("keys.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${key.id}`, () => mode === "local" ? relayCommands.deleteKey(key.id) : relayCommands.remoteAction({ type: "delete_key", id: key.id }), "feedback.deleted"))}>{t("keys.delete")}</ActionMenuItem>
      </ActionMenu></td>
    </tr>)}</tbody></table></div>
    {revealed ? <div className="one-time-secret" role="status"><strong>{t("keys.copyNow")}</strong><code>{revealed}</code><Button variant="secondary" onClick={() => navigator.clipboard.writeText(revealed)}>{t("common.copy")}</Button></div> : null}
    {editing ? <KeyPolicyDialog key={editing.id} value={editing} onClose={() => setEditing(null)} /> : null}
  </>;
}

function KeyPolicyDialog({ value, onClose }: { value: KeySummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const [label, setLabel] = useState(value.label);
  const [sourceIds, setSourceIds] = useState(value.sourceIds ?? []);
  const [accountIds, setAccountIds] = useState(value.accountIds ?? []);
  const [allowed, setAllowed] = useState(value.allowedModels.join(", "));
  const [excluded, setExcluded] = useState(value.excludedModels.join(", "));
  const [prefix, setPrefix] = useState(value.modelPrefix ?? "");
  const save = async () => {
    const payload = { label, sourceIds: sourceIds.length ? sourceIds : null, accountIds: accountIds.length ? accountIds : null, allowedModels: parseList(allowed), excludedModels: parseList(excluded), modelPrefix: prefix.trim() || null };
    const ok = await perform(`key-policy-${value.id}`, () => mode === "local" ? relayCommands.updateKey({ keyId: value.id, ...payload }) : relayCommands.remoteAction({ type: "update_key", id: value.id }, payload), "feedback.saved");
    if (ok) onClose();
  };
  const remove = async () => {
    if (!await confirm(t("keys.deleteConfirm"), { danger: true })) return;
    const ok = await perform(`delete-${value.id}`, () => mode === "local" ? relayCommands.deleteKey(value.id) : relayCommands.remoteAction({ type: "delete_key", id: value.id }), "feedback.deleted");
    if (ok) onClose();
  };
  return <Dialog wide title={t("keys.editPolicy")} onClose={onClose} footer={<><Button variant="danger" busy={busy === `delete-${value.id}`} onClick={() => void remove()}>{t("keys.delete")}</Button><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `key-policy-${value.id}`} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><p className="form-note">{t("keys.clientHint")}</p><label className="relay-field"><span>{t("keys.label")}</span><input value={label} onChange={(event) => setLabel(event.target.value)} /></label><fieldset><legend>{t("pool.members")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} />{account.label}</label>)}{runtime?.sources.map((source) => <label key={source.id}><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={() => setSourceIds(toggle(sourceIds, source.id))} />{source.name}</label>)}</div></fieldset><label className="relay-field"><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label className="relay-field"><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label><label className="relay-field"><span>{t("keys.modelPrefix")}</span><input value={prefix} onChange={(event) => setPrefix(event.target.value)} /></label></div></Dialog>;
}

function ModelsView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const models = runtime ? modelSummaries(runtime).sort(compareModelCatalog) : [];
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  if (!models.length) return <EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} />;
  return <section className="model-rules">
    <header>
      <div className="model-rules-copy"><h2>{t("models.visible")}</h2><p>{t("models.explanation")}</p></div>
    </header>
    <ul>{models.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const hasPrice = model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      return <li key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"}>
        <div className="model-identity"><code>{model.id}</code><StatusBadge status={model.enabled ? "ready" : "disabled"} label={t(model.enabled ? "models.available" : "models.disabled")} /></div>
        <div className="model-price">{hasPrice ? <><span>{t("models.inputPrice", { price: formatModelPrice(model.inputMicroUsdPerMillion!, i18n.language) })}</span><span>{t("models.outputPrice", { price: formatModelPrice(model.outputMicroUsdPerMillion!, i18n.language) })}</span><small>{t("models.perMillion")}</small></> : <span className="muted">{t("models.priceUnavailable")}</span>}</div>
        <span className="model-members">{t("pool.membersCount", { count: model.memberCount })}</span>
        <IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="relay-icon-button model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} />
      </li>;
    })}</ul>
  </section>;
}

function CreateKeyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [label, setLabel] = useState("ChatGPT");
  const [secret, setSecret] = useState("");
  useEffect(() => () => setSecret(""), []);
  const create = async () => { const result: { current: { secret: string } | null } = { current: null }; const ok = await perform("key-create", async () => { result.current = mode === "local" ? await relayCommands.createKey(label) : await relayCommands.remoteAction({ type: "create_key" }, { label, sourceIds: null, accountIds: null, allowedModels: [], excludedModels: [], modelPrefix: null }) as { secret: string }; }, "feedback.keyCreated"); if (ok && result.current) setSecret(result.current.secret); };
  return <Dialog title={t("keys.create")} onClose={onClose} footer={secret ? <Button variant="primary" onClick={onClose}>{t("common.done")}</Button> : <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "key-create"} onClick={create}>{t("keys.create")}</Button></>}>{secret ? <div className="one-time-secret"><strong>{t("keys.copyNow")}</strong><code>{secret}</code><Button variant="secondary" onClick={() => navigator.clipboard.writeText(secret)}>{t("common.copy")}</Button><p>{t("keys.shownOnce")}</p></div> : <label className="relay-field"><span>{t("keys.label")}</span><input value={label} onChange={(event) => setLabel(event.target.value)} /></label>}</Dialog>;
}

function parseList(value: string) { return [...new Set(value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean))]; }
function toggle(values: string[], value: string) { return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]; }
function formatApiEquivalent(microUsd: number, locale: string) { return `≈${new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(microUsd / 1_000_000)}`; }
function formatModelPrice(microUsd: number, locale: string) { return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 3 }).format(microUsd / 1_000_000); }
function modelSummaries(runtime: NonNullable<ReturnType<typeof useRelayState>["runtime"]>): ModelSummary[] {
  if (runtime.gateway.models?.length) return [...runtime.gateway.models];
  return runtime.gateway.visibleModelIds.map((id) => ({
    id,
    enabled: true,
    memberCount: [...runtime.sources, ...runtime.accounts].filter((member) => member.models.some((model) => model.toLowerCase() === id.toLowerCase())).length,
    catalogRank: null,
    inputMicroUsdPerMillion: null,
    outputMicroUsdPerMillion: null,
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
function memberQuota(member: Member) {
  if (member.kind === "source") return null;
  const values = [member.quota.primary, member.quota.secondary]
    .map((window) => window?.availableBasisPoints)
    .filter((value): value is number => value != null);
  return values.length ? Math.min(...values) : null;
}
function memberName(member: Member) { return member.kind === "source" ? member.name : member.label; }
function memberRoutingExcluded(member: Member) { return member.kind === "account" && member.routingExclusion != null; }
function poolMemberStatus(member: Member, runtimeState?: CandidateRuntimeSnapshot): "rotation" | "quotaWait" | "unavailable" | "disabled" {
  if (!member.enabled) return "disabled";
  if (runtimeState?.available) return "rotation";
  if (member.kind === "account" && (memberRoutingExcluded(member) || [member.quota.primary, member.quota.secondary].some((window) => window?.availableBasisPoints === 0))) return "quotaWait";
  if (runtimeState) return "unavailable";
  return poolMemberReady(member) ? "rotation" : "unavailable";
}
function poolMemberReady(member: Member) {
  if (!member.enabled || member.draining || !["unknown", "healthy", "degraded"].includes(member.health) || !member.secretAvailable || memberRoutingExcluded(member)) return false;
  return member.kind === "source" || (member.proxyAvailable !== false && ![member.quota.primary, member.quota.secondary].some((window) => window?.availableBasisPoints === 0));
}
