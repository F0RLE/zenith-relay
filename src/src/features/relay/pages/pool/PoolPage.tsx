import { useEffect, useState } from "react";
import { ArrowRightLeft, ArrowUpDown, CheckCheck, KeyRound, LayoutGrid, List, Loader2, Pencil, Play, Plus, Power, RefreshCw, RotateCcw, Rows3, Settings2, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, KeySummary, ModelSummary, SourceSummary } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, PageHeader, QuotaStack, StatusBadge, Tabs, accountPlanOption, apiSourcePriority, apiSourceRole, compareAccountPlans, formatAccountPlan, isCodexOauthAccountEligible } from "../../components/Ui";
import type { ApiSourceRole } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { SourceDialog } from "../connections/ConnectionsPage";

type View = "members" | "keys" | "models";
type MemberSort = "routing" | "quota" | "name";
type MemberLayout = "compact" | "list" | "grid";
type ModelSort = "catalog" | "price_desc" | "price_asc" | "name";
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
    ? <Button variant="primary" icon={<KeyRound aria-hidden />} disabled={!supportsKeys} title={!supportsKeys ? t("remote.capabilityUnavailable") : undefined} onClick={() => setCreateKey(true)}>{t("keys.create")}</Button>
    : view === "members"
      ? <Button variant="secondary" icon={<Plus aria-hidden />} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : undefined} onClick={() => setAddMembers(true)}>{t("pool.addMember")}</Button>
      : null;
  const viewMenuAction = view === "keys"
    ? <ActionMenuItem icon={<KeyRound aria-hidden />} disabled={!supportsKeys} title={!supportsKeys ? t("remote.capabilityUnavailable") : undefined} onClick={() => setCreateKey(true)}>{t("keys.create")}</ActionMenuItem>
    : view === "members"
      ? <ActionMenuItem icon={<Plus aria-hidden />} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : undefined} onClick={() => setAddMembers(true)}>{t("pool.addMember")}</ActionMenuItem>
      : null;
  const poolReady = Boolean(runtime?.gateway.candidateCount && runtime.gateway.visibleModelIds.length);
  const selectedOauthAccountId = codexPoolOauthSelection !== "none" && codexPoolOauthSelection !== "auto"
    && runtime?.accounts.some((account) => account.id === codexPoolOauthSelection && isCodexOauthAccountEligible(account))
    ? codexPoolOauthSelection
    : null;
  const switchCodexToPool = () => activateCodexProfile("pool-switch", async () => {
    const snapshot = await relayCommands.localState();
    const key = snapshot.keys.find((candidate) => candidate.enabled
      && !candidate.sourceIds?.length
      && !candidate.accountIds?.length
      && !candidate.allowedModels.length
      && !candidate.excludedModels.length
      && !candidate.modelPrefix)
      ?? (await relayCommands.createKey(t("pool.codexKeyLabel"))).key;
    return relayCommands.attachCodexGateway(key.id, selectedOauthAccountId, codexPoolOauthSelection === "none");
  }, true);
  const running = Boolean(runtime?.gateway.running);
  const action = mode === "local" ? <>
    {running
      ? <Button variant="primary" icon={<ArrowRightLeft aria-hidden />} busy={busy === "pool-switch"} disabled={!poolReady} title={!poolReady ? t("pool.startUnavailable") : undefined} onClick={() => void switchCodexToPool()}>{t("pool.switchCodex")}</Button>
      : <Button data-action="pool-toggle" variant="primary" icon={<Play aria-hidden />} busy={busy === "pool-toggle"} disabled={!poolReady} title={!poolReady ? t("pool.startUnavailable") : t("pool.start")} onClick={() => void perform("pool-toggle", relayCommands.startGateway, "feedback.started")}>{t("pool.start")}</Button>}
    <ActionMenu label={t("common.actions")}>
      {viewMenuAction}
      <ActionMenuItem icon={<Settings2 aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={() => setRoutingPolicy(true)}>{t("pool.routingSettings")}</ActionMenuItem>
      {running ? <ActionMenuItem icon={<Power aria-hidden />} disabled={busy === "pool-toggle"} onClick={() => void perform("pool-toggle", relayCommands.stopGateway, "feedback.stopped")}>{t("pool.stop")}</ActionMenuItem> : null}
    </ActionMenu>
  </> : <>
    {viewAction}
    <ActionMenu label={t("common.actions")}>
      <ActionMenuItem icon={<Settings2 aria-hidden />} disabled={!supportsRoutingSettings} title={!supportsRoutingSettings ? t("remote.capabilityUnavailable") : undefined} onClick={() => setRoutingPolicy(true)}>{t("pool.routingSettings")}</ActionMenuItem>
    </ActionMenu>
  </>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsKeys ? [{ id: "keys", label: t("pool.keys") }] : []), ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <MembersView onAdd={() => setAddMembers(true)} onQuotaPolicy={() => setQuotaPolicy(true)} /> : null}{view === "keys" ? <KeysView onCreate={() => setCreateKey(true)} /> : null}{view === "models" ? <ModelsView /> : null}{createKey ? <CreateKeyDialog onClose={() => setCreateKey(false)} /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{quotaPolicy ? <QuotaPolicyDialog onClose={() => setQuotaPolicy(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function MembersView({ onAdd, onQuotaPolicy }: { onAdd: () => void; onQuotaPolicy: () => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const canRefreshQuota = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("quota"));
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [sortBy, setSortBy] = useState<MemberSort>("routing");
  const [layout, setLayout] = useState<MemberLayout>(() => {
    const saved = localStorage.getItem("relay.pool.memberLayout");
    return saved === "compact" || saved === "grid" ? saved : "list";
  });
  useEffect(() => localStorage.setItem("relay.pool.memberLayout", layout), [layout]);
  const members: Member[] = [
    ...(runtime?.accounts ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "account" as const })),
    ...(runtime?.sources ?? []).filter((item) => item.inPool).map((item) => ({ ...item, kind: "source" as const, health: item.enabled ? "healthy" : "disabled", quota: null })),
  ].sort((left, right) => comparePoolMembers(left, right, sortBy));
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
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={onAdd}>{t("pool.addMember")}</Button>} />;
  const counts = { healthy: members.filter(poolMemberReady).length, limited: members.filter((item) => item.enabled && !poolMemberReady(item)).length, disabled: members.filter((item) => !item.enabled).length };
  return <>
    <div className="pool-controls">
      <div className="table-toolbar pool-member-toolbar">
        <label className="account-sort-select" title={t("pool.routingOrderHint")}><ArrowUpDown aria-hidden /><span>{t("pool.sortLabel")}</span><select aria-label={t("pool.sortLabel")} value={sortBy} onChange={(event) => setSortBy(event.target.value as MemberSort)}><option value="routing">{t("pool.sort.routing")}</option><option value="quota">{t("pool.sort.quota")}</option><option value="name">{t("pool.sort.name")}</option></select></label>
        <div className="inline-actions pool-quota-actions"><div className="view-layout-switcher" role="group" aria-label={t("pool.layout.label")}><IconButton label={t("pool.layout.compact")} aria-pressed={layout === "compact"} onClick={() => setLayout("compact")} icon={<Rows3 aria-hidden />} /><IconButton label={t("pool.layout.list")} aria-pressed={layout === "list"} onClick={() => setLayout("list")} icon={<List aria-hidden />} /><IconButton label={t("pool.layout.grid")} aria-pressed={layout === "grid"} onClick={() => setLayout("grid")} icon={<LayoutGrid aria-hidden />} /></div><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "pool-quota-refresh"} disabled={!canRefreshQuota || !quotaAccountCount} title={!quotaAccountCount ? t("pool.noQuotaMembers") : !canRefreshQuota ? t("remote.capabilityUnavailable") : undefined} onClick={() => void refreshQuotas()}>{t("pool.refreshQuotas")}</Button><IconButton label={t("pool.refreshPolicy")} icon={<Settings2 aria-hidden />} disabled={!canRefreshQuota} onClick={onQuotaPolicy} /></div>
      </div>
      <div className="pool-summary"><div><span>{t("pool.healthy")}</span><strong>{counts.healthy}</strong></div><div><span>{t("pool.limited")}</span><strong>{counts.limited}</strong></div><div><span>{t("common.disabled")}</span><strong>{counts.disabled}</strong></div></div>
    </div>
    <div className="pool-member-list" role="list" aria-label={t("pool.members")} data-layout={layout}>
      {members.map((member) => {
        const memberId = `${member.kind}:${member.id}`;
        const ready = poolMemberReady(member);
        const excludedByFreePolicy = member.kind === "account" && member.routingExclusion === "free_plan_policy";
        const statusKey = !member.enabled ? "disabled" : excludedByFreePolicy ? "freePolicy" : ready ? "ready" : "limited";
        const statusTone = !member.enabled ? "disabled" : ready ? "ready" : "warning";
        const identity = member.kind === "source" ? member.name : member.identityHint || member.label;
        const detail = member.kind === "source"
          ? `${member.wireApi} · ${member.baseUrl} · ${t(`sources.roles.${apiSourceRole(member.priority)}`)}`
          : [member.label, formatAccountPlan(member.subscription.planType, t("common.unknown")), member.priority !== 0 ? t("pool.priorityValue", { value: member.priority }) : null].filter(Boolean).join(" · ");
        const quota = memberQuota(member);
        const editLabel = `${t("pool.editMember")}: ${member.kind === "source" ? member.name : member.label}`;
        return <article key={`${member.kind}-${member.id}`} className={`pool-member-card${selectedId === memberId ? " selected" : ""}`} role="listitem" data-member-label={member.kind === "source" ? member.name : member.label}>
          <div className="pool-member-card-main">
            <div className="pool-member-state"><StatusBadge status={statusTone} label={t(`pool.memberStatus.${statusKey}`)} /><small>{t(`pool.types.${member.kind}`)}</small></div>
            <div className="pool-member-identity"><strong title={identity}>{identity}</strong><small title={detail}>{detail}</small></div>
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
  const [requestTimeoutSeconds, setRequestTimeoutSeconds] = useState(runtime?.gateway.quotaRequestTimeoutSeconds ?? 20);
  const [useFreeAccounts, setUseFreeAccounts] = useState(runtime?.gateway.useFreeAccounts ?? mode === "remote");
  const supportsFreePolicy = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("free_account_policy"));
  const save = async () => {
    const payload = { refreshIntervalSeconds, requestTimeoutSeconds, useFreeAccounts };
    const ok = await perform("quota-policy", () => mode === "local"
      ? relayCommands.updateQuotaPolicy(refreshIntervalSeconds, requestTimeoutSeconds, useFreeAccounts)
      : relayCommands.remoteAction({ type: "set_quota_policy" }, payload), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.refreshPolicyTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "quota-policy"} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("pool.refreshInterval")}</span><select value={refreshIntervalSeconds} onChange={(event) => setRefreshIntervalSeconds(Number(event.target.value))}><option value={120}>{t("pool.refreshIntervals.twoMinutes")}</option><option value={300}>{t("pool.refreshIntervals.fiveMinutes")}</option><option value={600}>{t("pool.refreshIntervals.tenMinutes")}</option><option value={1800}>{t("pool.refreshIntervals.thirtyMinutes")}</option><option value={3600}>{t("pool.refreshIntervals.oneHour")}</option></select></label><label className="relay-field"><span>{t("pool.requestTimeout")}</span><select value={requestTimeoutSeconds} onChange={(event) => setRequestTimeoutSeconds(Number(event.target.value))}><option value={10}>{t("pool.requestTimeouts.tenSeconds")}</option><option value={15}>{t("pool.requestTimeouts.fifteenSeconds")}</option><option value={20}>{t("pool.requestTimeouts.twentySeconds")}</option></select></label><label className="toggle-row"><input type="checkbox" checked={useFreeAccounts} disabled={!supportsFreePolicy} title={!supportsFreePolicy ? t("remote.capabilityUnavailable") : undefined} onChange={(event) => setUseFreeAccounts(event.target.checked)} /><span>{t("pool.useFreeAccounts")}</span></label><p className="form-note">{supportsFreePolicy ? t("pool.useFreeAccountsHint") : t("pool.useFreeAccountsLegacyHint")}</p><p className="form-note">{t("pool.refreshPolicyHint")}</p></div></Dialog>;
}

function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [maxRetryCandidates, setMaxRetryCandidates] = useState(runtime?.gateway.maxRetryCandidates ?? 3);
  const [sessionAffinity, setSessionAffinity] = useState(runtime?.gateway.sessionAffinity ?? true);
  const [sessionAffinityTtlSeconds, setSessionAffinityTtlSeconds] = useState(runtime?.gateway.sessionAffinityTtlSeconds ?? 3_600);
  const poolAccounts = (runtime?.accounts ?? []).filter((account) => account.inPool);
  const prioritiesVary = new Set(poolAccounts.map((account) => account.priority)).size > 1;
  const save = async () => {
    const payload = { maxRetryCandidates, sessionAffinity, sessionAffinityTtlSeconds };
    const ok = await perform("routing-policy", () => mode === "local"
      ? relayCommands.updateRouting(maxRetryCandidates, sessionAffinity, sessionAffinityTtlSeconds)
      : relayCommands.remoteAction({ type: "set_routing_policy" }, payload), "feedback.saved");
    if (ok) onClose();
  };
  const equalizePriorities = async () => {
    const targets = poolAccounts.filter((account) => account.priority !== 0);
    if (!targets.length || !window.confirm(t("pool.equalizePrioritiesConfirm"))) return;
    await perform("priority-equalize", async () => {
      for (const account of targets) {
        if (mode === "local") await relayCommands.updateAccount({ accountId: account.id, priority: 0 });
        else await relayCommands.remoteAction({ type: "update_account", id: account.id }, { priority: 0 });
      }
    }, "feedback.saved");
  };
  return <Dialog title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "routing-policy"} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="toggle-row" title={t("pool.sessionAffinityHelp")}><input type="checkbox" checked={sessionAffinity} onChange={(event) => setSessionAffinity(event.target.checked)} /><span>{t("pool.sessionAffinity")}</span></label><label className="relay-field"><span>{t("pool.sessionAffinityTtl")}</span><select value={sessionAffinityTtlSeconds} disabled={!sessionAffinity} onChange={(event) => setSessionAffinityTtlSeconds(Number(event.target.value))}><option value={60}>{t("pool.affinityDurations.oneMinute")}</option><option value={300}>{t("pool.affinityDurations.fiveMinutes")}</option><option value={900}>{t("pool.affinityDurations.fifteenMinutes")}</option><option value={3600}>{t("pool.affinityDurations.oneHour")}</option><option value={21600}>{t("pool.affinityDurations.sixHours")}</option><option value={86400}>{t("pool.affinityDurations.oneDay")}</option></select></label><label className="relay-field"><span>{t("pool.retryCandidates")}</span><select value={maxRetryCandidates} onChange={(event) => setMaxRetryCandidates(Number(event.target.value))}>{Array.from({ length: 8 }, (_, index) => index + 1).map((value) => <option key={value} value={value}>{value}</option>)}</select></label>{prioritiesVary ? <><p className="form-note routing-priority-note">{t("pool.priorityTiersWarning")}</p><Button variant="secondary" icon={<RotateCcw aria-hidden />} busy={busy === "priority-equalize"} onClick={() => void equalizePriorities()}>{t("pool.equalizePriorities")}</Button></> : null}{!sessionAffinity ? <p className="form-note">{t("pool.affinityDisabledWarning")}</p> : null}</div></Dialog>;
}

function MemberEditor({ member, onClose, onRemove }: { member: Member; onClose: () => void; onRemove: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [priority, setPriority] = useState(member.priority);
  const [sourceRole, setSourceRole] = useState<ApiSourceRole>(apiSourceRole(member.priority));
  const [weight, setWeight] = useState(member.weight);
  const [allowed, setAllowed] = useState(member.allowedModels.join(", "));
  const [excluded, setExcluded] = useState(member.excludedModels.join(", "));
  const [draining, setDraining] = useState(member.draining);
  const save = async () => {
    const allowedModels = parseList(allowed);
    const excludedModels = parseList(excluded);
    const resolvedPriority = member.kind === "source" ? apiSourcePriority(sourceRole) : priority;
    const payload = { priority: resolvedPriority, weight, allowedModels, excludedModels, draining };
    const ok = await perform(`member-${member.id}`, () => {
      if (member.kind === "account") return mode === "local"
        ? relayCommands.updateAccount({ accountId: member.id, priority: resolvedPriority, weight, allowedModels, excludedModels, draining })
        : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, allowedModels, excludedModels, draining, priority: resolvedPriority, weight };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide title={`${t("pool.editMember")} · ${member.kind === "source" ? member.name : member.label}`} onClose={onClose} footer={<><Button variant="danger" onClick={onRemove}>{t("pool.removeMember")}</Button><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={save}>{t("pool.savePolicy")}</Button></>}><div className="relay-form"><div className="settings-row">{member.kind === "source" ? <label><span>{t("sources.poolRole")}</span><select value={sourceRole} onChange={(event) => setSourceRole(event.target.value as ApiSourceRole)}><option value="primary">{t("sources.roles.primary")}</option><option value="stabilizer">{t("sources.roles.stabilizer")}</option><option value="reserve">{t("sources.roles.reserve")}</option></select><small>{t(`sources.roleHints.${sourceRole}`)}</small></label> : <label><span title={t("pool.priorityHelp")}>{t("pool.routingPriority")}</span><input type="number" value={priority} onChange={(event) => setPriority(Number(event.target.value))} /><small>{t("pool.priorityHelp")}</small></label>}<label><span title={t("pool.weightHelp")}>{t("pool.trafficShare")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label><label className="toggle-row"><input type="checkbox" checked={draining} onChange={(event) => setDraining(event.target.checked)} /><span>{t("accounts.drain")}</span></label></div><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} placeholder="gpt-5.4, gpt-5.4-mini" /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><p className="form-note">{t("pool.modelListHint")}</p></div></Dialog>;
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
        <div className="pool-member-options">{accounts.map((account) => { const plan = formatAccountPlan(account.subscription.planType, t("common.unknown")); return <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} /><span><strong>{account.identityHint || account.label}</strong><small>{account.label}</small></span><em data-plan={plan.toLocaleLowerCase()}>{plan}</em></label>; })}</div>
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
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("keys"));
  const [revealed, setRevealed] = useState("");
  const [editing, setEditing] = useState<KeySummary | null>(null);
  if (!runtime?.keys.length) return <EmptyState title={t("keys.emptyTitle")} description={t("keys.emptyDescription")} action={<Button variant="primary" disabled={!canManage} title={!canManage ? t("remote.capabilityUnavailable") : undefined} onClick={onCreate}>{t("keys.create")}</Button>} />;
  const formatLastUsed = new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" });
  return <>
    <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("keys.masked")}</th><th>{t("keys.scope")}</th><th>{t("common.models")}</th><th>{t("common.lastUsed")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead><tbody>{runtime.keys.map((key) => <tr key={key.id}>
      <td><StatusBadge status={key.enabled ? "ready" : "disabled"} label={key.enabled ? t("common.enabled") : t("common.disabled")} /></td>
      <td>{key.label}</td>
      <td><code>zlr_••••••••••••</code></td>
      <td>{(key.accountIds?.length || key.sourceIds?.length) ? t("keys.scoped") : t("keys.allMembers")}</td>
      <td>{key.allowedModels.length || t("keys.allModels")}</td>
      <td>{key.lastUsedAtMs ? formatLastUsed.format(new Date(key.lastUsedAtMs)) : t("common.never")}</td>
      <td className="row-actions"><IconButton label={t("keys.editPolicy")} icon={<Pencil aria-hidden />} onClick={() => setEditing(key)} /><ActionMenu>
        <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`enable-${key.id}`, () => mode === "local" ? relayCommands.setKeyEnabled(key.id, !key.enabled) : relayCommands.remoteAction({ type: "update_key", id: key.id }, { enabled: !key.enabled }), "feedback.saved")}>{key.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
        <ActionMenuItem icon={<RotateCcw aria-hidden />} disabled={busy === `rotate-${key.id}`} onClick={async () => {
          if (!window.confirm(t("keys.rotateConfirm"))) return;
          const result: { current: { secret: string } | null } = { current: null };
          await perform(`rotate-${key.id}`, async () => { result.current = mode === "local" ? await relayCommands.rotateKey(key.id) : await relayCommands.remoteAction({ type: "rotate_key", id: key.id }) as { secret: string }; }, "feedback.keyRotated");
          if (result.current) setRevealed(result.current.secret);
        }}>{t("keys.rotate")}</ActionMenuItem>
        <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("keys.deleteConfirm"))) void perform(`delete-${key.id}`, () => mode === "local" ? relayCommands.deleteKey(key.id) : relayCommands.remoteAction({ type: "delete_key", id: key.id }), "feedback.deleted"); }}>{t("keys.delete")}</ActionMenuItem>
      </ActionMenu></td>
    </tr>)}</tbody></table></div>
    {revealed ? <div className="one-time-secret" role="status"><strong>{t("keys.copyNow")}</strong><code>{revealed}</code><Button variant="secondary" onClick={() => navigator.clipboard.writeText(revealed)}>{t("common.copy")}</Button></div> : null}
    {editing ? <KeyPolicyDialog key={editing.id} value={editing} onClose={() => setEditing(null)} /> : null}
  </>;
}

function KeyPolicyDialog({ value, onClose }: { value: KeySummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
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
    if (!window.confirm(t("keys.deleteConfirm"))) return;
    const ok = await perform(`delete-${value.id}`, () => mode === "local" ? relayCommands.deleteKey(value.id) : relayCommands.remoteAction({ type: "delete_key", id: value.id }), "feedback.deleted");
    if (ok) onClose();
  };
  return <Dialog wide title={t("keys.editPolicy")} onClose={onClose} footer={<><Button variant="danger" busy={busy === `delete-${value.id}`} onClick={() => void remove()}>{t("keys.delete")}</Button><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `key-policy-${value.id}`} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><p className="form-note">{t("keys.clientHint")}</p><label className="relay-field"><span>{t("keys.label")}</span><input value={label} onChange={(event) => setLabel(event.target.value)} /></label><fieldset><legend>{t("pool.members")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} />{account.label}</label>)}{runtime?.sources.map((source) => <label key={source.id}><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={() => setSourceIds(toggle(sourceIds, source.id))} />{source.name}</label>)}</div></fieldset><label className="relay-field"><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label className="relay-field"><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label><label className="relay-field"><span>{t("keys.modelPrefix")}</span><input value={prefix} onChange={(event) => setPrefix(event.target.value)} /></label></div></Dialog>;
}

function ModelsView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [sortBy, setSortBy] = useState<ModelSort>("catalog");
  const models = runtime ? modelSummaries(runtime).sort((left, right) => compareModels(left, right, sortBy)) : [];
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
      <label className="model-sort-select"><span>{t("models.sortLabel")}</span><span className="model-sort-control"><ArrowUpDown aria-hidden /><select aria-label={t("models.sortLabel")} value={sortBy} onChange={(event) => setSortBy(event.target.value as ModelSort)}><option value="catalog">{t("models.sort.catalog")}</option><option value="price_desc">{t("models.sort.priceDesc")}</option><option value="price_asc">{t("models.sort.priceAsc")}</option><option value="name">{t("models.sort.name")}</option></select></span></label>
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
  const [label, setLabel] = useState("Codex");
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
function compareModels(left: ModelSummary, right: ModelSummary, sortBy: ModelSort) {
  if (sortBy === "name") return left.id.localeCompare(right.id);
  if (sortBy === "price_desc") return compareModelPrice(left, right, -1) || compareModelCatalog(left, right);
  if (sortBy === "price_asc") return compareModelPrice(left, right, 1) || compareModelCatalog(left, right);
  return compareModelCatalog(left, right);
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
function comparePoolMembers(left: Member, right: Member, sortBy: MemberSort) {
  if (sortBy === "name") return memberName(left).localeCompare(memberName(right));
  if (sortBy === "quota") return comparePoolQuota(right, left) || right.priority - left.priority || memberName(left).localeCompare(memberName(right));
  return Number(memberRoutingExcluded(left)) - Number(memberRoutingExcluded(right)) || right.priority - left.priority || comparePoolQuota(right, left) || right.weight - left.weight || memberName(left).localeCompare(memberName(right));
}
function comparePoolQuota(left: Member, right: Member) {
  const leftQuota = memberQuota(left);
  const rightQuota = memberQuota(right);
  if (leftQuota == null && rightQuota == null) return 0;
  if (leftQuota == null) return -1;
  if (rightQuota == null) return 1;
  return leftQuota - rightQuota;
}
function memberQuota(member: Member) {
  if (member.kind === "source") return null;
  const values = [member.quota.primary, member.quota.secondary]
    .map((window) => window?.availableBasisPoints)
    .filter((value): value is number => value != null);
  return values.length ? Math.min(...values) : null;
}
function memberName(member: Member) { return member.kind === "source" ? member.name : member.label; }
function memberRoutingExcluded(member: Member) { return member.kind === "account" && member.routingExclusion != null; }
function poolMemberReady(member: Member) {
  if (!member.enabled || member.draining || member.health !== "healthy" || !member.secretAvailable || memberRoutingExcluded(member)) return false;
  return member.kind === "source" || (member.proxyAvailable !== false && ![member.quota.primary, member.quota.secondary].some((window) => window?.availableBasisPoints === 0));
}
