import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, CheckCheck, Layers, Plus, Search, Server, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { AccountPlanBadge, Button, Dialog, EmptyState, OptionMenu } from "../../components/Ui";
import { accountPlanOption, compareAccountPlans } from "../../routingOrder";
import { compareStableText, toggle } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";

type MemberView = "all" | "accounts" | "sources" | "selected";

function MemberOption({ name, detail, icon, badge, checked, disabled, onChange }: {
  name: string; detail?: string; icon: ReactNode; badge?: ReactNode;
  checked: boolean; disabled: boolean; onChange: () => void;
}) {
  return <label className="pool-picker-option" data-selected={checked}>
    <span className="pool-picker-avatar" aria-hidden>{icon}</span>
    <span className="pool-member-option-copy"><strong>{name}</strong>{detail ? <small>{detail}</small> : null}</span>
    {badge}
    <input type="checkbox" aria-label={name} checked={checked} disabled={disabled} onChange={onChange} />
  </label>;
}

export function AddMembersDialog({ onClose, onAddSource }: { onClose: () => void; onAddSource: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canAddSource = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("sources"));
  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [sourceIds, setSourceIds] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<MemberView>("all");
  const [planFilter, setPlanFilter] = useState("all");
  const listRef = useRef<HTMLDivElement>(null);
  const allAccounts = (runtime?.accounts ?? []).filter((account) => !account.inPool);
  const allSources = (runtime?.sources ?? []).filter((source) => !source.inPool);
  const planOptions = new Map<string, { id: string; label: string; count: number }>();
  for (const account of allAccounts) {
    const option = accountPlanOption(account.subscription.planType, t("common.unknown"));
    const current = planOptions.get(option.id);
    planOptions.set(option.id, { ...option, count: (current?.count ?? 0) + 1 });
  }
  const plans = [...planOptions.values()].sort(compareAccountPlans);
  const activePlan = view !== "accounts" || !planOptions.has(planFilter) ? "all" : planFilter;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matches = (...values: Array<string | null | undefined>) => !normalizedQuery || values.some((value) => value?.toLocaleLowerCase().includes(normalizedQuery));
  const accounts = allAccounts
    .filter((account) => view !== "sources" && (view !== "selected" || accountIds.includes(account.id)))
    .filter((account) => activePlan === "all" || accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan)
    .filter((account) => matches(account.identityHint, account.label, account.subscription.planType))
    .sort((left, right) => compareAccountPlans(accountPlanOption(left.subscription.planType, t("common.unknown")), accountPlanOption(right.subscription.planType, t("common.unknown"))) || compareStableText(left.label, right.label));
  const sources = allSources
    .filter((source) => view !== "accounts" && (view !== "selected" || sourceIds.includes(source.id)) && matches(source.name, source.baseUrl))
    .sort((left, right) => compareStableText(left.name, right.name));
  // Do not submit members that were removed or added elsewhere during a refresh.
  const selectedAccounts = accountIds.filter((id) => allAccounts.some((account) => account.id === id));
  const selectedSources = sourceIds.filter((id) => allSources.some((source) => source.id === id));
  const selectedCount = selectedAccounts.length + selectedSources.length;
  const availableCount = allAccounts.length + allSources.length;
  const shownCount = accounts.length + sources.length;
  const checkedCount = accounts.filter((account) => accountIds.includes(account.id)).length + sources.filter((source) => sourceIds.includes(source.id)).length;
  const shownSelected = shownCount > 0 && checkedCount === shownCount;
  const toggleShown = () => {
    const update = (current: string[], visible: string[]) => shownSelected
      ? current.filter((id) => !visible.includes(id))
      : [...new Set([...current, ...visible])];
    setAccountIds((current) => update(current, accounts.map((account) => account.id)));
    setSourceIds((current) => update(current, sources.map((source) => source.id)));
  };
  useEffect(() => { listRef.current?.scrollTo({ top: 0 }); }, [view, query, activePlan]);
  const add = async () => {
    const ok = await perform("pool-add-members", () => updatePoolMembership(mode, { accountIds: selectedAccounts, sourceIds: selectedSources, inPool: true }), "feedback.saved");
    if (ok) onClose();
  };
  const saving = busy === "pool-add-members";
  const views: Array<{ id: MemberView; label: string; icon: ReactNode }> = [
    { id: "all", label: t("pool.allConnections"), icon: <Layers aria-hidden /> },
    { id: "accounts", label: t("connections.accounts"), icon: <UserRound aria-hidden /> },
    { id: "sources", label: t("connections.sources"), icon: <Server aria-hidden /> },
    { id: "selected", label: t("pool.selectedMembers"), icon: <CheckCheck aria-hidden /> },
  ];
  return <Dialog className="pool-add-dialog" title={t("pool.addMembersTitle")} onClose={onClose} footer={<>
    <div className="pool-picker-summary" role="status"><span className="pool-picker-summary-icon" data-active={selectedCount > 0}><Check aria-hidden /></span><span>{selectedCount ? t("pool.selectionCount", { count: selectedCount }) : t("pool.chooseMembers")}</span></div>
    <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
    <Button variant="primary" busy={saving} disabled={!selectedCount} aria-label={t("pool.addSelected", { count: selectedCount })} onClick={add}>{t("pool.confirmAdd")}</Button>
  </>}>
    <div className="pool-member-picker">
      <aside className="pool-picker-sidebar">
        <nav aria-label={t("pool.memberTypes")}>
          {views.map((item) => <button key={item.id} type="button" aria-pressed={view === item.id} onClick={() => setView(item.id)}>{item.icon}<span>{item.label}</span>{item.id === "selected" && selectedCount ? <small>{selectedCount}</small> : null}</button>)}
        </nav>
        <Button className="pool-picker-new" variant="ghost" icon={<Plus aria-hidden />} aria-label={t("sources.addToPool")} disabled={!canAddSource || saving} title={!canAddSource ? t("remote.capabilityUnavailable") : undefined} onClick={onAddSource}>{t("pool.newSource")}</Button>
      </aside>
      <div className="pool-picker-content">
        <div className="pool-picker-toolbar">
          <label className="pool-picker-search"><Search aria-hidden /><input type="search" aria-label={t("pool.searchMembers")} value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("pool.searchMembersPlaceholder")} /></label>
          {view === "accounts" && plans.length > 1 ? <OptionMenu className="pool-picker-plan" label={t("accounts.filterByPlan")} value={activePlan} options={[
            { value: "all", label: t("accounts.allPlans") },
            ...plans.map((plan) => ({ value: plan.id, label: plan.label })),
          ]} onChange={setPlanFilter} /> : null}
        </div>
        <div className="pool-picker-selection">
          <label><input type="checkbox" ref={(node) => { if (node) node.indeterminate = checkedCount > 0 && !shownSelected; }} checked={shownSelected} disabled={!shownCount || saving} onChange={toggleShown} /><span>{t("pool.selectVisible")}</span></label>
          {selectedCount ? <Button variant="ghost" disabled={saving} onClick={() => { setAccountIds([]); setSourceIds([]); }}>{t("accounts.clearSelection")}</Button> : null}
        </div>
        <div ref={listRef} className="pool-picker-list">
          {accounts.length ? <section aria-label={t("connections.accounts")} className="pool-member-options">
            {accounts.map((account) => <MemberOption key={account.id} name={account.label} icon={<UserRound />} badge={<AccountPlanBadge planType={account.subscription.planType} unknown={t("common.unknown")} />} checked={accountIds.includes(account.id)} disabled={saving} onChange={() => setAccountIds((current) => toggle(current, account.id))} />)}
          </section> : null}
          {sources.length ? <section aria-label={t("connections.sources")} className="pool-member-options">
            {sources.map((source) => <MemberOption key={source.id} name={source.name} detail={source.baseUrl} icon={<Server />} checked={sourceIds.includes(source.id)} disabled={saving} onChange={() => setSourceIds((current) => toggle(current, source.id))} />)}
          </section> : null}
          {!availableCount ? <EmptyState title={t("pool.noAvailableMembers")} description={t("pool.noAvailableMembersHint")} /> : !shownCount ? <EmptyState title={t(view === "selected" && !selectedCount ? "pool.noSelectedMembers" : "pool.noMatchingMembers")} description={t(view === "selected" && !selectedCount ? "pool.noSelectedMembersHint" : "pool.noMatchingMembersHint")} /> : null}
        </div>
      </div>
    </div>
  </Dialog>;
}
