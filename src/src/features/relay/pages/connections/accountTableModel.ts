import type { AccountSummary } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import { accountPlanOption, compareAccountPlans, compareRoutingOrder } from "../../routingOrder";
import { compareStableText } from "../../poolHelpers";
import { matchesQuery } from "./connectionHelpers";

export type ParticipationFilter = "all" | "included" | "excluded";
export type AccountPlanOption = { id: string; label: string; count: number };

export function accountParticipates(account: Pick<AccountSummary, "inPool">) {
  return account.inPool;
}

export function accountCounts(accounts: readonly AccountSummary[]) {
  return {
    errorCount: accounts.filter((account) => Boolean(currentAccountErrorCode(account))).length,
    inPoolCount: accounts.filter(accountParticipates).length,
    disabledCount: accounts.filter((account) => !account.enabled).length,
  };
}

export function accountPlanOptions(accounts: readonly AccountSummary[], unknown: string) {
  const options = new Map<string, AccountPlanOption>();
  for (const account of accounts) {
    const option = accountPlanOption(account.subscription.planType, unknown);
    const current = options.get(option.id);
    options.set(option.id, { ...option, count: (current?.count ?? 0) + 1 });
  }
  return [...options.values()].sort(compareAccountPlans);
}

export function activeAccountPlan(planFilter: string, plans: readonly AccountPlanOption[], errorCount: number) {
  return planFilter === "all"
    || plans.some((plan) => plan.id === planFilter)
    || (planFilter === "errors" && errorCount > 0)
    ? planFilter
    : "all";
}

export function filterAndSortAccounts(
  accounts: readonly AccountSummary[],
  query: string,
  activePlan: string,
  participationFilter: ParticipationFilter,
  groupByPlan: boolean,
  runtimePosition: ReadonlyMap<string, number>,
  unknown: string,
) {
  return [...accounts]
    .filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType, account.models))
    .filter((account) => activePlan === "all"
      || (activePlan === "errors" ? Boolean(currentAccountErrorCode(account)) : accountPlanOption(account.subscription.planType, unknown).id === activePlan))
    .filter((account) => participationFilter === "all" || (participationFilter === "included") === accountParticipates(account))
    .sort((left, right) => groupByPlan
      ? compareAccountPlans(accountPlanOption(left.subscription.planType, unknown), accountPlanOption(right.subscription.planType, unknown))
        || compareRoutingOrder(left.id, right.id, runtimePosition)
        || compareStableText(left.identityHint || left.label, right.identityHint || right.label)
      : compareRoutingOrder(left.id, right.id, runtimePosition)
        || compareStableText(left.identityHint || left.label, right.identityHint || right.label));
}

export function visiblePlanCounts(accounts: readonly AccountSummary[], unknown: string) {
  return accounts.reduce((counts, account) => {
    const id = accountPlanOption(account.subscription.planType, unknown).id;
    counts.set(id, (counts.get(id) ?? 0) + 1);
    return counts;
  }, new Map<string, number>());
}

export function accountSelectionState(
  allAccounts: readonly AccountSummary[],
  visibleAccounts: readonly AccountSummary[],
  selectedIds: readonly string[],
) {
  const selected = new Set(selectedIds);
  const selectedAccounts = visibleAccounts.filter((account) => selected.has(account.id));
  const selectedAccountIds = selectedAccounts.map((account) => account.id);
  const selectedCount = selectedAccounts.length;
  const selectedOnServer = selectedAccounts.some((account) => Boolean(account.remoteLocation));
  return {
    selectedAccounts,
    selectedIds: selectedAccountIds,
    selectedCount,
    selectedAccessOnly: selectedAccounts.some((account) => account.authState.state === "degraded_access_only"),
    selectedSecretsUnavailable: selectedAccounts.some((account) => !account.secretAvailable),
    selectedOnServer,
    exportIds: selectedCount ? selectedAccountIds : allAccounts.map((account) => account.id),
    canIncludeSelected: !selectedOnServer && selectedAccounts.some((account) => !accountParticipates(account)),
    canExcludeSelected: selectedAccounts.some(accountParticipates),
    allSelected: visibleAccounts.length > 0 && visibleAccounts.every((account) => selected.has(account.id)),
  };
}
