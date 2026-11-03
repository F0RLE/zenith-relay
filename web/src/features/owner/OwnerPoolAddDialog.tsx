import { useEffect, useRef, useState } from "react";
import { accountPlanOption, compareAccountPlans } from "../../../../src/src/features/relay/accountPlans";
import { compareStableText, toggle } from "../../../../src/src/features/relay/poolHelpers";
import { Dialog } from "../../components/Dialog";
import { Check, CheckCheck, Layers2, Plus, Search, Server, UserRound } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import type { ConnectionAction, OwnerConnection } from "./model";
import { OwnerSourceDialog } from "./OwnerSourceDialog";
import { sourceEndpoint } from "./sourcePresentation";
import "./owner-pool-add-dialog.css";

type MemberView = "all" | "accounts" | "sources" | "selected";

function memberStatus(connection: OwnerConnection): { tone: string; label: MessageKey } {
  if (connection.paused) return { tone: "disabled", label: "connections.suspended" };
  if (connection.account?.quotaStatus === "requires_reauth") return { tone: "error", label: "account.signInAgain" };
  if (connection.account?.quotaStatus === "failed") return { tone: "error", label: "account.couldNotRefreshQuotas" };
  // A saved, unpooled connection is not evidence that it can serve requests.
  return { tone: "idle", label: "connections.outsidePool" };
}

/** Desktop AddMembersDialog composition, using only Hosted's safe inventory. */
export function OwnerPoolAddDialog({ connections, change, onClose, onAdded }: {
  connections: OwnerConnection[]; change: (action: ConnectionAction) => void;
  onClose: () => void; onAdded: () => void;
}) {
  const { t, locale } = useLocale();
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [view, setView] = useState<MemberView>("all");
  const [query, setQuery] = useState("");
  const [plan, setPlan] = useState("all");
  const [creatingSource, setCreatingSource] = useState(false);
  const [focusNewSource, setFocusNewSource] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  const available = connections.filter(item => !item.inPool && !item.trashed);
  const selected = available.filter(item => selectedIds.includes(item.id));
  const planFor = (item: OwnerConnection) => accountPlanOption(item.account?.plan ?? null, t("common.unknown"));
  const plans = [...new Map(available.filter(item => item.kind === "account").map(item => {
    const option = planFor(item); return [option.id, option] as const;
  })).values()].sort(compareAccountPlans);
  const activePlan = view === "accounts" && plans.some(option => option.id === plan) ? plan : "all";
  const needle = query.trim().toLocaleLowerCase(locale);
  const visible = available.filter(item => (view === "all" || view === "selected" && selectedIds.includes(item.id)
    || view === "accounts" && item.kind === "account" || view === "sources" && item.kind === "api")
    && (activePlan === "all" || planFor(item).id === activePlan)
    && (!needle || [item.name, sourceEndpoint(item.source?.baseUrl), item.account?.plan, item.account?.route]
      .some(value => value?.toLocaleLowerCase(locale).includes(needle))));
  const accounts = visible.filter(item => item.kind === "account").sort((a, b) => compareAccountPlans(planFor(a), planFor(b)) || compareStableText(a.name, b.name));
  const sources = visible.filter(item => item.kind === "api").sort((a, b) => compareStableText(a.name, b.name));
  const checkedCount = visible.filter(item => selectedIds.includes(item.id)).length;
  const shownSelected = visible.length > 0 && checkedCount === visible.length;
  useEffect(() => { listRef.current?.scrollTo({ top: 0 }); }, [view, query, activePlan, creatingSource]);

  if (creatingSource) return <OwnerSourceDialog change={action => {
    change(action);
    if (action.type === "saveSource") {
      setSelectedIds(ids => [...new Set([...ids, action.id])]);
      setView("selected"); setQuery(""); setFocusNewSource(false);
    }
  }} onClose={() => setCreatingSource(false)} />;

  const views = [
    { id: "all", label: "pool.allConnections", Icon: Layers2 },
    { id: "accounts", label: "pool.accountConnections", Icon: UserRound },
    { id: "sources", label: "pool.apiSources", Icon: Server },
    { id: "selected", label: "pool.selectedMembers", Icon: CheckCheck },
  ] as const;
  const option = (item: OwnerConnection) => {
    const status = memberStatus(item);
    const endpoint = item.kind === "api" ? sourceEndpoint(item.source?.baseUrl) : null;
    const statusLabel = t(status.label);
    return <label className="owner-pool-picker-option" key={item.id} data-kind={item.kind} data-selected={selectedIds.includes(item.id)}>
      <span className="owner-pool-picker-avatar" data-status={status.tone} aria-hidden="true">{item.kind === "account" ? <UserRound /> : <Server />}</span>
      <span className="owner-pool-picker-copy"><strong title={item.name}>{item.name}</strong><small data-status={status.tone}>{statusLabel}</small>{endpoint && <small title={endpoint}>{endpoint}</small>}</span>
      {item.kind === "account" && <span className="owner-pool-picker-badges"><span>{planFor(item).label}</span>{item.account?.route && <span className="owner-pool-picker-route">{item.account.route}</span>}</span>}
      <input type="checkbox" aria-label={`${item.name}, ${statusLabel}`} checked={selectedIds.includes(item.id)} onChange={() => setSelectedIds(ids => toggle(ids, item.id))} />
    </label>;
  };
  return <Dialog className="owner-pool-add-dialog" title={t("pool.addMembersTitle")} onClose={onClose}>
    <div className="owner-pool-member-picker">
      <aside className="owner-pool-picker-sidebar">
        <nav aria-label={t("pool.memberTypes")}>{views.map(({ id, label, Icon }) => <button type="button" key={id} aria-pressed={view === id} onClick={() => setView(id)}><Icon aria-hidden="true" /><span>{t(label)}</span>{id === "selected" && selected.length > 0 && <small>{selected.length.toLocaleString(locale)}</small>}</button>)}</nav>
        <button type="button" className="owner-pool-picker-new" data-initial-focus={focusNewSource || undefined} onClick={() => { setFocusNewSource(true); setCreatingSource(true); }}><Plus aria-hidden="true" /><span>{t("pool.newSource")}</span></button>
      </aside>
      <div className="owner-pool-picker-content">
        <div className="owner-pool-picker-toolbar">
          <label className="owner-pool-picker-search"><Search aria-hidden="true" /><input type="search" data-initial-focus={!focusNewSource || undefined} aria-label={t("pool.searchMembers")} placeholder={t("pool.searchMembersPlaceholder")} value={query} onChange={event => setQuery(event.target.value)} /></label>
          {view === "accounts" && plans.length > 1 && <OptionMenu label={t("pool.filterByPlan")} value={activePlan} options={[{ value: "all", label: t("pool.allPlans") }, ...plans.map(option => ({ value: option.id, label: option.label }))]} onChange={setPlan} />}
        </div>
        <div className="owner-pool-picker-selection">
          <label><input type="checkbox" ref={node => { if (node) node.indeterminate = checkedCount > 0 && !shownSelected; }} checked={shownSelected} disabled={!visible.length} onChange={() => setSelectedIds(ids => shownSelected ? ids.filter(id => !visible.some(item => item.id === id)) : [...new Set([...ids, ...visible.map(item => item.id)])])} /><span>{t("pool.selectVisible")}</span></label>
          {selected.length > 0 && <button type="button" onClick={() => setSelectedIds([])}>{t("pool.clearSelection")}</button>}
        </div>
        <div ref={listRef} className="owner-pool-picker-list">
          {accounts.length > 0 && <section className="owner-pool-member-options" aria-label={t("pool.accountConnections")}>{accounts.map(option)}</section>}
          {sources.length > 0 && <section className="owner-pool-member-options" aria-label={t("pool.apiSources")}>{sources.map(option)}</section>}
          {!visible.length && <p className="owner-pool-picker-empty" role="status">{t(!available.length ? "pool.noConnectionsOutsideThePool" : view === "selected" && !selected.length ? "pool.noSelectedMembers" : "pool.noMatchingMembers")}</p>}
        </div>
      </div>
    </div>
    <footer className="owner-pool-picker-footer">
      <div className="owner-pool-picker-summary" role="status"><span className="owner-pool-picker-summary-icon" data-active={selected.length > 0}><Check aria-hidden="true" /></span><span>{selected.length ? t("pool.selectionCount", { count: selected.length.toLocaleString(locale) }) : t("pool.chooseMembers")}</span></div>
      <button type="button" className="button" onClick={onClose}>{t("common.cancel")}</button>
      <button type="button" className="button primary-button" disabled={!selected.length} onClick={() => { change({ type: "addMembers", ids: selected.map(item => item.id) }); onAdded(); }}>{t("connections.addToPool")}</button>
    </footer>
  </Dialog>;
}
