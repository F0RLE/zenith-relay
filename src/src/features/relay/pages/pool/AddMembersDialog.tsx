import { useState } from "react";
import { CheckCheck, Plus, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { AccountPlanBadge, Button, Dialog, EmptyState } from "../../components/Ui";
import { accountPlanOption, apiSourceRole, compareAccountPlans } from "../../routingOrder";
import { compareStableText, toggle } from "../../poolHelpers";
import { useRelayState } from "../../state/RelayStateProvider";

export function AddMembersDialog({ onClose, onAddSource }: { onClose: () => void; onAddSource: () => void }) {
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
