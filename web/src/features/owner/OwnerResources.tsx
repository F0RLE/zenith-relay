import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Layers2, ListMinus, ListPlus, Pause, Play, Plus, RefreshCw, RotateCcw, Search, SlidersHorizontal, Trash, Users } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { OptionMenu } from "../../components/OptionMenu";
import type { CustomerModel } from "../models/ModelsPage";
import type { ConnectionAction, OwnerConnection } from "./model";
import { OwnerResourceCard } from "./OwnerResourceCard";
import { ResourceSelection } from "./ResourceSelection";
import { OwnerResourceSummary } from "./OwnerResourceSummary";
import { OwnerSourceDialog } from "./OwnerSourceDialog";
import { OwnerPoolAddDialog } from "./OwnerPoolAddDialog";
import { OwnerProxies } from "./OwnerProxies";
import { OwnerPoolRotation } from "./OwnerPoolRotation";
import { OwnerPoolMemberEditor } from "./OwnerPoolMemberEditor";
import { OwnerPoolRuntime, OwnerPoolSpeedControl } from "./OwnerPoolRuntime";
import { OwnerModelRulesView } from "./OwnerModelRules";
import type { OwnerModelRules } from "./modelRules";
import type { OwnerPoolSpeed } from "./OwnerPoolRuntime";
import { poolActivityKey, projectPoolActivity } from "./poolActivity";
import type { OwnerPoolActivity } from "./poolActivity";
import { movePoolRotationMember, reconcilePoolRotation } from "./poolRotation";
import type { PoolRoutingPolicy } from "./poolRotation";
import type { OwnerProxy } from "./proxyPresentation";
import type { ResourceStat } from "./OwnerResourceSummary";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import type { Locale } from "../../i18n/catalog";

type ResourceProps = {
  connections: OwnerConnection[];
  models: CustomerModel[];
  change: (action: ConnectionAction) => void;
  active?: boolean;
  asOf: string;
  proxies?: OwnerProxy[];
  onProxiesChange?: (proxies: OwnerProxy[]) => void;
  poolActivity?: OwnerPoolActivity | undefined;
};

function BulkAction({ label, icon, disabled = false, danger = false, onClick }: {
  label: MessageKey; icon: ReactNode; disabled?: boolean; danger?: boolean; onClick: () => void;
}) {
  const { t } = useLocale();
  return <button type="button" className={`relay-button${danger ? " danger-button" : ""}`} aria-label={t(label)} data-relay-tooltip={t(label)} disabled={disabled} onClick={onClick}>{icon}<span>{t(label)}</span></button>;
}

function ResourceControls({ stats, accounts, nowMs, valuesVisible, onToggleValues, query, onQuery, filter, onFilter, options, visible, selected, onSelectAll, children, pool }: {
  stats: ResourceStat[]; accounts: OwnerConnection[]; nowMs: number; valuesVisible: boolean; onToggleValues: () => void;
  query: string; onQuery: (value: string) => void;
  filter: string; onFilter: (value: string) => void;
  options: { value: string; label: MessageKey }[];
  visible: number; selected: number; onSelectAll: () => void; children?: ReactNode;
  pool?: { context: ReactNode; actions: ReactNode };
}) {
  const { t, locale } = useLocale();
  const controls = useRef<HTMLElement>(null);
  const previousSelected = useRef(selected);
  const filterId = useId();
  const [searchOpen, setSearchOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  useEffect(() => { if (searchOpen) searchRef.current?.focus(); }, [searchOpen]);
  useEffect(() => {
    if (previousSelected.current > 0 && selected === 0 && document.activeElement === document.body && controls.current?.getClientRects().length) {
      controls.current.querySelector<HTMLInputElement>(pool && !searchOpen ? 'input[type="checkbox"]' : 'input[type="search"]')?.focus();
    }
    previousSelected.current = selected;
  }, [selected]);
  return <section ref={controls} className="owner-resource-controls" aria-label={t("connections.manageConnections")}>
    <OwnerResourceSummary stats={stats} accounts={accounts} nowMs={nowMs} valuesVisible={valuesVisible} />
    <div className={`owner-resource-toolbar${pool ? " owner-pool-toolbar" : ""}`}>
      <div className="owner-resource-selection">
        <ResourceSelection label={t("connections.selectAllVisibleConnections")} checked={visible > 0 && selected === visible} mixed={selected > 0 && selected < visible} disabled={!visible} onChange={onSelectAll} />
        {selected ? <span role="status">{t("connections.selectedCount", { count: selected.toLocaleString(locale) })}</span> : pool?.context ?? <span>{t("connections.selectAll")}</span>}
      </div>
      <div className="owner-resource-actions-slot" data-selecting={selected > 0 || undefined}>
        {pool ? <div className="owner-pool-actions" inert={selected > 0} aria-hidden={selected > 0 || undefined}>
          {pool.actions}
          <button className="relay-icon-button" type="button" aria-label={t("pool.searchMembers")} data-relay-tooltip={t("pool.searchMembers")} aria-expanded={searchOpen} aria-controls={filterId} onClick={() => { if (searchOpen) { onQuery(""); onFilter("all"); } setSearchOpen(value => !value); }}><Search aria-hidden="true" /></button>
          <button className="relay-icon-button owner-values-toggle" type="button" aria-label={t("connections.showAccountEstimates")} data-relay-tooltip={t(valuesVisible ? "connections.hideAccountEstimates" : "connections.showAccountEstimates")} aria-pressed={valuesVisible} onClick={onToggleValues}><span aria-hidden="true">$</span></button>
          <span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t("connections.refreshUnavailable")}><button className="relay-icon-button" type="button" aria-label={t("connections.refreshConnections")} disabled><RefreshCw aria-hidden="true" /></button></span>
        </div> : <div className="owner-resource-filters" inert={selected > 0} aria-hidden={selected > 0 || undefined}>
          <label className="owner-resource-search"><Search aria-hidden="true" /><input type="search" aria-label={t("connections.searchConnections")} placeholder={t("connections.findAConnection")} value={query} onChange={event => onQuery(event.target.value)} /></label>
          <OptionMenu label={t("connections.connectionState")} value={filter} onChange={onFilter} options={options.map(item => ({ ...item, label: t(item.label) }))} />
          <button className="relay-icon-button owner-values-toggle" type="button" aria-label={t("connections.showAccountEstimates")} data-relay-tooltip={t(valuesVisible ? "connections.hideAccountEstimates" : "connections.showAccountEstimates")} aria-pressed={valuesVisible} onClick={onToggleValues}><span aria-hidden="true">$</span></button>
          <span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t("connections.refreshUnavailable")}><button className="relay-icon-button" type="button" aria-label={t("connections.refreshConnections")} disabled><RefreshCw aria-hidden="true" /></button></span>
        </div>}
        {selected > 0 && <div className="owner-resource-bulk" role="group" aria-label={t("connections.actionsForSelectedConnections")}>{children}</div>}
      </div>
      {pool && <div className="owner-pool-filter-row" id={filterId} hidden={!searchOpen} inert={selected > 0}>
        <label className="owner-resource-search"><Search aria-hidden="true" /><input ref={searchRef} type="search" aria-label={t("connections.searchConnections")} placeholder={t("connections.findAConnection")} value={query} onChange={event => onQuery(event.target.value)} /></label>
        <OptionMenu label={t("connections.connectionState")} value={filter} onChange={onFilter} options={options.map(item => ({ ...item, label: t(item.label) }))} />
      </div>}
    </div>
  </section>;
}

function useSelection(visible: OwnerConnection[]) {
  const [ids, setIds] = useState<string[]>([]);
  const selected = visible.filter(item => ids.includes(item.id));
  return {
    selected,
    clear: () => setIds([]),
    toggle: (id: string) => setIds(current => current.includes(id) ? current.filter(value => value !== id) : [...current, id]),
    toggleAll: () => setIds(selected.length === visible.length ? [] : visible.map(item => item.id)),
  };
}

function matches(connection: OwnerConnection, query: string, models: CustomerModel[], locale: Locale, localize: (value: string) => string) {
  return `${connection.name} ${connection.source?.baseUrl ?? ""} ${connection.family} ${localize(connection.family)} ${connection.models.map(id => models.find(model => model.id === id)?.name ?? id).join(" ")}`.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale));
}

export function ConnectionsPage({ connections, models, change, active = true, asOf, proxies = [], onProxiesChange }: ResourceProps) {
  const { t, locale, localize } = useLocale();
  const [tab, setTab] = useState("account");
  const [valuesVisible, setValuesVisible] = useState(true);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [dialog, setDialog] = useState<"add" | { id: string } | null>(null);
  const [sourceEditor, setSourceEditor] = useState<{ id?: string; section?: "connection" | "models" } | null>(null);
  const [name, setName] = useState("");
  const [modelIds, setModelIds] = useState<string[]>([]);
  const [proxyImportOpen, setProxyImportOpen] = useState(false);
  const inventory = connections.filter(item => !item.trashed);
  const visible = connections.filter(item => (tab === "trash" ? item.trashed : !item.trashed && item.kind === tab)
    && matches(item, query, models, locale, localize)
    && (filter === "all" || filter === "pool" && item.inPool || filter === "outside" && !item.inPool || filter === "paused" && item.paused));
  const selection = useSelection(visible);
  const detail = dialog && dialog !== "add" ? connections.find(item => item.id === dialog.id) : undefined;
  useEffect(() => { if (!active) { setDialog(null); setSourceEditor(null); selection.clear(); } }, [active]);
  function bulk(type: "pool" | "trash" | "restore", inPool?: boolean) {
    for (const item of selection.selected) if (type !== "pool" || item.inPool !== inPool) change({ type, id: item.id });
    selection.clear();
  }
  return <>
    <header className="relay-page-header relay-workspace-header"><h1>{t("nav.connections")}</h1>
      <div className="relay-page-navigation"><div className="relay-tabs" role="group" aria-label={t("connections.connectionType")}>{([["account", "connections.accounts"], ["api", "connections.apiProviders"], ["proxy", "ownerProxies.title"], ["trash", "connections.trash"]] as const).map(([id, label]) => <button key={id} className={tab === id ? "active" : ""} type="button" aria-pressed={tab === id} onClick={() => { setTab(id); setFilter("all"); setDialog(null); setSourceEditor(null); setProxyImportOpen(false); selection.clear(); }}>{t(label)}</button>)}</div></div>
      <div className="relay-page-actions"><button className="relay-button primary" type="button" onClick={() => { if (tab === "proxy") { setProxyImportOpen(true); return; } if (tab !== "account") { setSourceEditor({}); return; } setName(""); setModelIds(models.map(model => model.id)); setDialog("add"); }}><Plus aria-hidden="true" />{t(tab === "proxy" ? "ownerProxies.import" : "connections.addConnection")}</button></div>
    </header>
    {tab === "proxy" ? <OwnerProxies entries={proxies} accounts={connections.filter(item => item.kind === "account" && !item.trashed)} onChange={onProxiesChange} active={active} importOpen={proxyImportOpen} onImportClose={() => setProxyImportOpen(false)} onImport={() => setProxyImportOpen(true)} /> : <>
    <ResourceControls stats={[{ label: "connections.total", value: inventory.length, icon: <Users aria-hidden="true" /> }, { label: "connections.inPool", value: inventory.filter(item => item.inPool).length, icon: <Layers2 aria-hidden="true" /> }, { label: "connections.paused", value: inventory.filter(item => item.paused).length, icon: <Pause aria-hidden="true" /> }, { label: "connections.inTrash", value: connections.length - inventory.length, icon: <Trash aria-hidden="true" /> }]}
      accounts={inventory} nowMs={Date.parse(asOf)} valuesVisible={valuesVisible} onToggleValues={() => setValuesVisible(value => !value)}
      query={query} onQuery={value => { setQuery(value); selection.clear(); }} filter={filter} onFilter={value => { setFilter(value); selection.clear(); }}
      options={tab === "trash" ? [{ value: "all", label: "connections.allStates" }] : [{ value: "all", label: "connections.allStates" }, { value: "pool", label: "connections.inPool" }, { value: "outside", label: "connections.outsidePool" }, { value: "paused", label: "connections.paused" }]}
      visible={visible.length} selected={selection.selected.length} onSelectAll={selection.toggleAll}>
      {tab === "trash" ? <BulkAction label="connections.restoreOutsidePool" icon={<RotateCcw aria-hidden="true" />} onClick={() => bulk("restore")} /> : <>
        <BulkAction label="connections.addToPool" icon={<ListPlus aria-hidden="true" />} disabled={selection.selected.every(item => item.inPool)} onClick={() => bulk("pool", true)} />
        <BulkAction label="connections.removeFromPool" icon={<ListMinus aria-hidden="true" />} disabled={selection.selected.every(item => !item.inPool)} onClick={() => bulk("pool", false)} />
        <BulkAction label="connections.moveToTrash" icon={<Trash aria-hidden="true" />} danger onClick={() => bulk("trash")} />
      </>}
    </ResourceControls>
    <div className="owner-resource-grid">{visible.map(connection => <OwnerResourceCard key={connection.id} connection={connection} models={models} nowMs={Date.parse(asOf)} active={active} change={change} valuesVisible={valuesVisible} selected={selection.selected.some(item => item.id === connection.id)} onSelect={() => selection.toggle(connection.id)} onDetails={() => setDialog({ id: connection.id })} onEditSource={() => setSourceEditor({ id: connection.id })} onSourceModels={connection.trashed ? undefined : () => setSourceEditor({ id: connection.id, section: "models" })} />)}</div>
    {!visible.length && <p className="owner-resource-empty" role="status">{t(query || filter !== "all" ? "connections.noConnectionsFound" : tab === "trash" ? "connections.trashIsEmpty" : "connections.noSavedConnections")}</p>}
    </>}
    {dialog === "add" && <Dialog title={t("connections.addConnection")} onClose={() => setDialog(null)}><form className="demo-form" onSubmit={event => {
      event.preventDefault(); if (!name.trim() || !modelIds.length) return;
      change({ type: "add", connection: { id: crypto.randomUUID(), name: name.trim(), kind: "account", family: "common.account", models: modelIds, inPool: false, paused: false, trashed: false } });
      setTab("account"); setQuery(""); setFilter("all"); selection.clear(); setDialog(null);
    }}>
      <label>{t("common.name")}<input required data-initial-focus maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
      <fieldset className="hosted-fieldset"><legend>{t("nav.models")}</legend>{models.map(model => <label className="checkbox-row" key={model.id}><input type="checkbox" checked={modelIds.includes(model.id)} onChange={event => setModelIds(current => event.target.checked ? [...current, model.id] : current.filter(id => id !== model.id))} />{model.name}</label>)}</fieldset>
      <button type="submit" className="button primary-button" disabled={!name.trim() || !modelIds.length}>{t("common.add")}</button>
    </form></Dialog>}
    {sourceEditor && <OwnerSourceDialog connection={connections.find(item => item.id === sourceEditor.id)} initialSection={sourceEditor.section} change={action => { change(action); setTab("api"); setQuery(""); setFilter("all"); selection.clear(); }} onClose={() => setSourceEditor(null)} />}
    {detail && <Dialog title={detail.name} onClose={() => setDialog(null)}>
      <dl className="detail-list"><div><dt>{t("connections.source")}</dt><dd>{localize(detail.family)}</dd></div><div><dt>{t("nav.models")}</dt><dd>{detail.models.map(id => models.find(model => model.id === id)?.name ?? id).join(", ")}</dd></div><div><dt>{t("nav.pool")}</dt><dd>{t(detail.inPool ? "connections.participating" : "connections.notParticipating")}</dd></div></dl>
      <p className="form-note">{t("connections.detailsUnavailable")}</p>
      <div className="button-row">{detail.trashed ? <button className="button" type="button" onClick={() => { change({ type: "restore", id: detail.id }); setDialog(null); }}>{t("connections.restoreOutsidePool")}</button> : <>
        <button className="button" type="button" onClick={() => { change({ type: "pool", id: detail.id }); setDialog(null); }}>{t(detail.inPool ? "connections.removeFromPool" : "connections.addToPool")}</button>
        <button className="button" type="button" onClick={() => { change({ type: "pause", id: detail.id }); setDialog(null); }}>{t(detail.paused ? "connections.resume" : "connections.pause")}</button>
        <button className="button danger-button" type="button" onClick={() => { change({ type: "trash", id: detail.id }); setDialog(null); }}>{t("connections.moveToTrash")}</button>
      </>}</div>
    </Dialog>}
  </>;
}

export function PoolPage({ connections, models, change, active = true, asOf, poolActivity, modelRules, onModelRulesChange, modelDisplayOrder, onModelOrderChange }: ResourceProps & { modelRules: OwnerModelRules; onModelRulesChange: (rules: OwnerModelRules) => void; modelDisplayOrder: string[]; onModelOrderChange: (order: string[]) => void }) {
  const { t, locale, localize } = useLocale();
  const [tab, setTab] = useState<"members" | "models">("members");
  const [rotationOpen, setRotationOpen] = useState(false);
  const [serviceTier, setServiceTier] = useState<OwnerPoolSpeed>("standard");
  const [valuesVisible, setValuesVisible] = useState(true);
  const [add, setAdd] = useState(false);
  const [memberEditorId, setMemberEditorId] = useState<string | null>(null);
  const [sourceEditor, setSourceEditor] = useState<{ id: string; section?: "connection" | "models" } | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [rotationPolicy, setRotationPolicy] = useState<PoolRoutingPolicy>(() => reconcilePoolRotation({ version: 3, mode: "automatic", members: [] }, connections));
  useEffect(() => { setRotationPolicy(current => reconcilePoolRotation(current, connections)); }, [connections]);
  const currentRotation = reconcilePoolRotation(rotationPolicy, connections);
  const members = currentRotation.members.flatMap(rule => {
    const member = connections.find(connection => connection.id === rule.id && (connection.kind === "api" ? "source" : "account") === rule.kind);
    return member ? [member] : [];
  });
  const candidates = connections.filter(connection => !connection.inPool && !connection.trashed);
  const activity = projectPoolActivity(members, poolActivity);
  const visible = members.filter(item => matches(item, query, models, locale, localize) && (filter === "all" || filter === "paused" && item.paused || filter === "enabled" && !item.paused));
  const selection = useSelection(visible);
  const editedMember = members.find(member => member.id === memberEditorId && member.kind === "account");
  useEffect(() => { if (!active) { setAdd(false); setRotationOpen(false); setSourceEditor(null); setMemberEditorId(null); selection.clear(); } }, [active]);
  function bulk(type: "pool" | "pause", paused?: boolean) {
    for (const item of selection.selected) if (type !== "pause" || item.paused !== paused) change({ type, id: item.id });
    selection.clear();
  }
  function changePool(action: ConnectionAction) {
    if (action.type !== "move") { change(action); return; }
    setRotationPolicy(current => {
      const policy = reconcilePoolRotation(current, connections);
      const index = policy.members.findIndex(member => member.id === action.id);
      return movePoolRotationMember(policy, index, index + action.direction);
    });
  }
  return <>
    <header className="relay-page-header relay-workspace-header"><h1>{t("nav.pool")}</h1>
      <div className="relay-page-navigation"><div className="relay-tabs" role="group" aria-label={t("nav.pool")}>{(["members", "models"] as const).map(value => <button key={value} type="button" className={tab === value ? "active" : ""} aria-pressed={tab === value} onClick={() => { setTab(value); selection.clear(); }}>{t(value === "members" ? "pool.members" : "modelRules.title")}</button>)}</div></div>
      {tab === "members" && <div className="relay-page-actions"><button type="button" className="relay-button primary" onClick={() => setAdd(true)}><Plus aria-hidden="true" />{t("pool.addMember")}</button></div>}
    </header>
    <div hidden={tab !== "members"}>
      <ResourceControls stats={[{ label: "pool.memberCount", value: members.length, icon: <Users aria-hidden="true" /> }, { label: "pool.enabled", value: members.filter(item => !item.paused).length, icon: <Play aria-hidden="true" /> }, { label: "connections.paused", value: members.filter(item => item.paused).length, icon: <Pause aria-hidden="true" /> }, { label: "connections.outsidePool", value: candidates.length, icon: <ListMinus aria-hidden="true" /> }]}
        pool={{ context: <OwnerPoolRuntime activity={activity} mode={currentRotation.mode} />, actions: <><OwnerPoolSpeedControl value={serviceTier} onChange={setServiceTier} /><button className="relay-icon-button" type="button" aria-haspopup="dialog" aria-label={t("pool.routingSettings")} data-relay-tooltip={t("pool.routingSettings")} onClick={() => setRotationOpen(true)}><SlidersHorizontal aria-hidden="true" /></button></> }}
        accounts={members} nowMs={Date.parse(asOf)} valuesVisible={valuesVisible} onToggleValues={() => setValuesVisible(value => !value)}
        query={query} onQuery={value => { setQuery(value); selection.clear(); }} filter={filter} onFilter={value => { setFilter(value); selection.clear(); }}
        options={[{ value: "all", label: "connections.allStates" }, { value: "enabled", label: "pool.enabled" }, { value: "paused", label: "connections.paused" }]}
        visible={visible.length} selected={selection.selected.length} onSelectAll={selection.toggleAll}>
        <BulkAction label="connections.pause" icon={<Pause aria-hidden="true" />} disabled={selection.selected.every(item => item.paused)} onClick={() => bulk("pause", true)} />
        <BulkAction label="connections.resume" icon={<Play aria-hidden="true" />} disabled={selection.selected.every(item => !item.paused)} onClick={() => bulk("pause", false)} />
        <BulkAction label="connections.removeFromPool" icon={<ListMinus aria-hidden="true" />} onClick={() => bulk("pool")} />
      </ResourceControls>
      <div className="owner-resource-grid">{visible.map(member => {
        const index = members.findIndex(item => item.id === member.id);
        return <OwnerResourceCard key={member.id} connection={member} models={models} nowMs={Date.parse(asOf)} active={active} change={changePool} valuesVisible={valuesVisible} position={index} totalMembers={members.length} allowReorder={currentRotation.mode !== "automatic"} currentRequests={activity.memberRequests.get(poolActivityKey(member)) ?? 0} selected={selection.selected.some(item => item.id === member.id)} onSelect={() => selection.toggle(member.id)} onEditMember={() => setMemberEditorId(member.id)} onEditSource={() => setSourceEditor({ id: member.id })} onSourceModels={() => setSourceEditor({ id: member.id, section: "models" })} />;
      })}</div>
      {!visible.length && <p className="owner-resource-empty" role="status">{t(query || filter !== "all" ? "pool.noMembersFound" : "pool.empty")}</p>}
    </div>
    <div hidden={tab !== "models"}><OwnerModelRulesView models={models} connections={connections} rules={modelRules} onChange={onModelRulesChange} displayOrder={modelDisplayOrder} onOrderChange={onModelOrderChange} active={active && tab === "models"} /></div>
    {rotationOpen && <Dialog title={t("pool.routingSettings")} className="owner-rotation-dialog" onClose={() => setRotationOpen(false)}><OwnerPoolRotation embedded policy={currentRotation} connections={connections} onChange={setRotationPolicy} /></Dialog>}
    {editedMember && <OwnerPoolMemberEditor key={editedMember.id} member={editedMember} change={change} onClose={() => setMemberEditorId(null)} />}
    {sourceEditor && <OwnerSourceDialog connection={connections.find(item => item.id === sourceEditor.id)} initialSection={sourceEditor.section} change={change} onClose={() => setSourceEditor(null)} />}
    {add && <OwnerPoolAddDialog connections={connections} change={change} onClose={() => setAdd(false)} onAdded={() => { setQuery(""); setFilter("all"); selection.clear(); setAdd(false); }} />}
  </>;
}
