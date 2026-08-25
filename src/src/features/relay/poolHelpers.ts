import type {
  AccountSummary,
  ModelSummary,
  RuntimeSnapshot,
  SourceSummary,
} from "./api/types";
import {
  accountPlanOption,
  apiSourceRole,
  compareRoutingOrder,
  compareSubscriptionPlanPriority,
  type ApiSourceRole,
} from "./routingOrder";
import { groupModels, modelProviderGroup, modelProviderGroupLabel } from "./modelGroups";

export type PoolMember =
  | (AccountSummary & { kind: "account" })
  | (SourceSummary & { kind: "source" });

export type SubscriptionPlanGroup = {
  id: string;
  label: string;
  count: number;
};

export type SourceRoutingStage = {
  role: ApiSourceRole | "accounts";
  count: number;
};

export function sourceOrderForRole(
  sources: SourceSummary[],
  role: ApiSourceRole,
  sourceId: string,
) {
  const current = sources.find((source) => source.id === sourceId);
  const ordered = sources
    .filter(
      (source) =>
        source.inPool &&
        source.id !== sourceId &&
        apiSourceRole(source.priority) === role,
    )
    .sort(compareSources);

  if (!current) return ordered.map((source) => source.id);
  ordered.push(current);
  if (apiSourceRole(current.priority) === role) ordered.sort(compareSources);
  return ordered.map((source) => source.id);
}

export function sourceRoutingStages(
  sources: SourceSummary[],
  accounts: AccountSummary[],
  sourceId: string,
  selectedRole: ApiSourceRole,
): SourceRoutingStage[] {
  const enabledSources = sources.filter(
    (source) => source.inPool && source.enabled,
  );
  const countForRole = (role: ApiSourceRole) =>
    enabledSources.filter(
      (source) =>
        (source.id === sourceId ? selectedRole : apiSourceRole(source.priority)) ===
        role,
    ).length;
  const enabledAccounts = accounts.filter(
    (account) => account.inPool && account.enabled,
  ).length;

  return [
    { role: "primary", count: countForRole("primary") },
    { role: "accounts", count: enabledAccounts },
    { role: "stabilizer", count: countForRole("stabilizer") },
    { role: "reserve", count: countForRole("reserve") },
  ];
}

export function subscriptionPlanGroups(
  accounts: AccountSummary[],
  unknown: string,
): SubscriptionPlanGroup[] {
  const groups = new Map<string, SubscriptionPlanGroup>();
  for (const account of accounts) {
    if (!account.inPool) continue;
    const id = account.subscription.planType?.trim().toLocaleLowerCase() || "unknown";
    const current = groups.get(id);
    if (current) current.count += 1;
    else groups.set(id, {
      id,
      label: accountPlanOption(account.subscription.planType, unknown).label,
      count: 1,
    });
  }
  return [...groups.values()].sort((left, right) =>
    compareSubscriptionPlanPriority(
      accountPlanOption(left.id === "unknown" ? null : left.id, unknown),
      accountPlanOption(right.id === "unknown" ? null : right.id, unknown),
    ),
  );
}

export function mergeSubscriptionPlanOrder(
  groups: SubscriptionPlanGroup[],
  saved: string[],
) {
  const available = new Set(groups.map((group) => group.id));
  return [
    ...saved.filter((plan) => available.delete(plan)),
    ...groups.map((group) => group.id).filter((plan) => available.has(plan)),
  ];
}

export function modelSummaries(runtime: RuntimeSnapshot): ModelSummary[] {
  if (runtime.gateway.models?.length) {
    return runtime.gateway.models.map((model) => ({
      ...model,
      codexVisible: model.codexVisible ?? false,
      codexDisplayName: model.codexDisplayName || model.id,
      reasoningLevels: model.reasoningLevels ?? [],
      reasoningSupportedLevels: model.reasoningSupportedLevels ?? [],
      reasoningAllowedLevels: model.reasoningAllowedLevels ?? [],
      reasoningConfigurable: model.reasoningConfigurable ?? false,
      speedSupported: model.speedSupported ?? false,
      speedTier: model.speedTier ?? "standard",
      speedConfigurable: model.speedConfigurable ?? false,
    }));
  }

  return runtime.gateway.visibleModelIds.map((id) => ({
    id,
    enabled: true,
    memberCount: [...runtime.sources, ...runtime.accounts].filter((member) =>
      member.models.some((model) => model.toLowerCase() === id.toLowerCase()),
    ).length,
    codexVisible: false,
    codexDisplayName: id,
    catalogRank: null,
    inputMicroUsdPerMillion: null,
    cachedInputMicroUsdPerMillion: null,
    cacheWrite5mMicroUsdPerMillion: null,
    cacheWrite1hMicroUsdPerMillion: null,
    outputMicroUsdPerMillion: null,
    imageRequestPrices: [],
    customPrice: false,
    reasoningLevels: [],
    reasoningSupportedLevels: [],
    reasoningAllowedLevels: [],
    reasoningConfigurable: false,
    speedSupported: false,
    speedTier: "standard",
    speedConfigurable: false,
  }));
}

export function groupModelSummariesForLauncher(
  models: ModelSummary[],
  accounts: AccountSummary[],
) {
  const chatGptModelIds = new Set(
    accounts.flatMap((account) => account.models.map((model) => model.toLowerCase())),
  );
  return groupModels(
    models,
    (model) => model.id,
    (model) => chatGptModelIds.has(model.id.toLowerCase()),
  );
}

/** Rules editor ordering is operator-owned. Keep the catalog order supplied by
 * the backend instead of applying launcher presentation sorting. */
export function groupModelSummariesForRules(
  models: ModelSummary[],
  accounts: AccountSummary[],
) {
  const nativeIds = new Set(
    accounts.flatMap((account) => account.models.map((model) => model.toLowerCase())),
  );
  const groups = new Map<string, ModelSummary[]>();
  for (const model of models) {
    const id = modelProviderGroup(model.id, nativeIds.has(model.id.toLowerCase()));
    const items = groups.get(id);
    if (items) items.push(model);
    else groups.set(id, [model]);
  }
  const groupOrder = (id: string) => {
    if (id === "chatgpt") return 0;
    if (id === "openai") return 1;
    if (id === "anthropic") return 2;
    if (id.startsWith("provider-")) return 3;
    return 4;
  };
  return [...groups.entries()]
    .sort(([left], [right]) => groupOrder(left) - groupOrder(right))
    .map(([id, items]) => ({
      id,
      label: modelProviderGroupLabel(id as Parameters<typeof modelProviderGroupLabel>[0]),
      items,
    }));
}

export function comparePoolMembers(
  left: PoolMember,
  right: PoolMember,
  order: Map<string, number>,
) {
  return (
    unavailable(left) - unavailable(right) ||
    compareRoutingOrder(left.id, right.id, order) ||
    compareStableText(memberName(left), memberName(right))
  );
}

export function memberName(member: PoolMember) {
  return member.kind === "source" ? member.name : member.identityHint || member.label;
}

export function toggle(values: string[], value: string) {
  return values.includes(value)
    ? values.filter((item) => item !== value)
    : [...values, value];
}

export function clampRoutingCount(value: string) {
  return Math.min(8, Math.max(1, Math.trunc(Number(value)) || 1));
}

function compareSources(left: SourceSummary, right: SourceSummary) {
  return (
    right.priority - left.priority ||
    compareStableText(left.name, right.name) ||
    compareStableText(left.id, right.id)
  );
}

function unavailable(member: PoolMember) {
  return member.operationalStatus === "unavailable" ||
    member.operationalStatus === "disabled"
    ? 1
    : 0;
}

export function compareStableText(left: string, right: string) {
  return left === right ? 0 : left < right ? -1 : 1;
}
