import type {
  AccountSummary,
  CandidateRuntimeSnapshot,
  ModelSummary,
  RuntimeSnapshot,
  SourceProtocolBinding,
  SourceSummary,
} from "./api/types";
import {
  accountPlanOption,
  compareRoutingOrder,
  compareSubscriptionPlanPriority,
} from "./routingOrder";
import { groupModels } from "./modelGroups";
import { compareOperationalStatus } from "./accountStatus";
import {
  effectiveSourceProtocolBindings,
  normalizedAdapter,
} from "./sourceProtocolBindings";

export type PoolMember =
  | (AccountSummary & { kind: "account" })
  | (SourceSummary & { kind: "source" });

export type SubscriptionPlanGroup = {
  id: string;
  label: string;
  count: number;
};

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
  // `gateway.models` is a derived projection and can lag a source/account
  // refresh. Merge it with the catalogs carried by the current members even
  // when the derived array is non-empty. Keep the gateway order (it already
  // contains the saved presentation order), then append newly observed IDs.
  const summaries = new Map<string, ModelSummary>();
  const order: string[] = [];
  const catalog = new Map(
    Object.entries(runtime.gateway.modelCatalog ?? {}).map(([id, identity]) => [id.toLowerCase(), identity]),
  );
  const add = (id: string, summary?: ModelSummary) => {
    const normalized = id.trim().toLowerCase();
    if (!normalized) return;
    if (!summaries.has(normalized)) order.push(normalized);
    const next = summary ? normalizeModelSummary(summary) : fallbackModelSummary(id.trim());
    const identity = catalog.get(normalized);
    // A source/account model can arrive before the derived gateway row. Keep
    // its catalog provider so the pool view still groups it correctly.
    if (identity && !next.catalogProvider) {
      next.catalogProvider = identity.catalogProvider ?? null;
      next.catalogFamily = identity.catalogFamily ?? null;
    }
    const existing = summaries.get(normalized);
    summaries.set(normalized, existing ? mergeModelSummary(existing, next) : next);
  };

  for (const model of runtime.gateway.models ?? []) add(model.id, model);
  for (const id of runtime.gateway.visibleModelIds) add(id);
  for (const source of runtime.sources) {
    for (const id of source.models) add(id);
    // A partially migrated source can have the model only on its binding.
    for (const binding of source.protocolBindings ?? []) {
      for (const id of binding.modelIds) add(id);
    }
  }
  for (const account of runtime.accounts) {
    for (const id of account.models) add(id);
  }

  const memberCount = new Map<string, number>();
  for (const member of [...runtime.sources, ...runtime.accounts]) {
    const ids = new Set(member.models.map((id) => id.trim().toLowerCase()).filter(Boolean));
    for (const id of ids) memberCount.set(id, (memberCount.get(id) ?? 0) + 1);
  }

  return order.map((id) => {
    const model = summaries.get(id)!;
    const count = memberCount.get(id);
    return count == null ? model : { ...model, memberCount: count };
  });
}

/**
 * Return the complete model inventory that can be ordered for this pool.
 *
 * The rules page edits this complete pool inventory. Runtime availability is
 * shown by member/runtime surfaces and must not remove a model from the
 * saved order merely because a route is temporarily unavailable.
 */
export function currentPoolModelSummaries(runtime: RuntimeSnapshot): ModelSummary[] {
  const memberCounts = new Map<string, number>();
  const addMember = (ids: string[]) => {
    for (const id of new Set(ids.map((id) => id.trim().toLowerCase()).filter(Boolean))) {
      memberCounts.set(id, (memberCounts.get(id) ?? 0) + 1);
    }
  };
  for (const source of runtime.sources) {
    if (!source.inPool) continue;
    // Keep the complete ordering inventory consistent with modelSummaries:
    // a partially migrated source can carry an ID only on its route binding.
    addMember([...source.models, ...(source.protocolBindings ?? []).flatMap((binding) => binding.modelIds)]);
  }
  for (const account of runtime.accounts) {
    if (!account.inPool) continue;
    addMember(account.models);
  }
  return modelSummaries(runtime)
    .filter((model) => memberCounts.has(model.id.trim().toLowerCase()))
    .map((model) => ({ ...model, memberCount: memberCounts.get(model.id.trim().toLowerCase())! }));
}

function normalizeModelSummary(model: ModelSummary): ModelSummary {
  return {
    ...model,
    codexVisible: model.codexVisible ?? false,
    codexDisplayName: model.codexDisplayName || model.id,
    reasoningLevels: model.reasoningLevels ?? [],
    reasoningSupportedLevels: model.reasoningSupportedLevels ?? [],
    reasoningAllowedLevels: model.reasoningAllowedLevels ?? [],
    reasoningConfigurable: model.reasoningConfigurable ?? false,
    speedSupported: model.speedSupported ?? false,
    speedTiers: model.speedTiers ?? [],
    speedTier: model.speedTier ?? "standard",
    speedConfigurable: model.speedConfigurable ?? false,
  };
}

function mergeModelSummary(existing: ModelSummary, incoming: ModelSummary): ModelSummary {
  const merged = { ...existing };
  const preferIncoming = <T>(current: T | null | undefined, next: T | null | undefined) =>
    current == null || current === "" ? next : current;
  const unionArray = <T>(current: readonly T[] | undefined, next: readonly T[] | undefined) =>
    [...new Set([...(current ?? []), ...(next ?? [])])];
  const unionReasoningLevels = (current: readonly string[] | undefined, next: readonly string[] | undefined) =>
    unionArray(current, next).sort((left, right) => {
      const rank = (value: string) => ({ none: 0, minimal: 1, low: 2, medium: 3, high: 4, xhigh: 5, max: 6, ultra: 7 }[value.replace("-", "_").toLowerCase()] ?? 8);
      return rank(left) - rank(right);
    });
  const unionImagePrices = (current: ModelSummary["imageRequestPrices"], next: ModelSummary["imageRequestPrices"]) => {
    const prices = new Map<string, NonNullable<ModelSummary["imageRequestPrices"]>[number]>();
    for (const price of [...(current ?? []), ...(next ?? [])]) {
      const key = `${price.operation}:${price.quality}:${price.size}:${price.microUsd}`;
      if (!prices.has(key)) prices.set(key, price);
    }
    return [...prices.values()];
  };
  const mergeProtocolRoutes = (
    current: ModelSummary["protocolRoutes"],
    next: ModelSummary["protocolRoutes"],
  ): NonNullable<ModelSummary["protocolRoutes"]> => {
    const routes = new Map<string, NonNullable<ModelSummary["protocolRoutes"]>[number]>();
    for (const route of [...(current ?? []), ...(next ?? [])]) {
      const key = `${route.clientWireApi}:${route.upstreamWireApi}`;
      const previous = routes.get(key);
      if (!previous) {
        routes.set(key, {
          ...route,
          features: { ...route.features },
          reasoningEfforts: [...route.reasoningEfforts],
        });
        continue;
      }
      routes.set(key, {
        ...previous,
        features: { ...previous.features, ...route.features },
        reasoningEfforts: unionReasoningLevels(previous.reasoningEfforts, route.reasoningEfforts),
      });
    }
    return [...routes.values()];
  };

  merged.memberCount = Math.max(existing.memberCount, incoming.memberCount);
  merged.protocolRoutes = mergeProtocolRoutes(existing.protocolRoutes, incoming.protocolRoutes);
  merged.codexDisplayName = existing.codexDisplayName === existing.id
    ? incoming.codexDisplayName
    : existing.codexDisplayName;
  merged.catalogProvider = preferIncoming(existing.catalogProvider, incoming.catalogProvider) ?? null;
  merged.catalogFamily = preferIncoming(existing.catalogFamily, incoming.catalogFamily) ?? null;
  merged.catalogName = preferIncoming(existing.catalogName, incoming.catalogName) ?? null;
  merged.catalogReleaseDate = preferIncoming(existing.catalogReleaseDate, incoming.catalogReleaseDate) ?? null;
  merged.catalogLastUpdated = preferIncoming(existing.catalogLastUpdated, incoming.catalogLastUpdated) ?? null;
  merged.catalogStatus = preferIncoming(existing.catalogStatus, incoming.catalogStatus) ?? null;
  merged.catalogReasoning = preferIncoming(existing.catalogReasoning, incoming.catalogReasoning) ?? null;
  merged.catalogReasoningMethod = preferIncoming(existing.catalogReasoningMethod, incoming.catalogReasoningMethod) ?? null;
  merged.catalogReasoningEffortLevels = unionReasoningLevels(existing.catalogReasoningEffortLevels, incoming.catalogReasoningEffortLevels);
  merged.catalogDefaultReasoningEffort = preferIncoming(existing.catalogDefaultReasoningEffort, incoming.catalogDefaultReasoningEffort) ?? null;
  merged.catalogToolCall = preferIncoming(existing.catalogToolCall, incoming.catalogToolCall) ?? null;
  merged.catalogStructuredOutput = preferIncoming(existing.catalogStructuredOutput, incoming.catalogStructuredOutput) ?? null;
  merged.catalogAttachment = preferIncoming(existing.catalogAttachment, incoming.catalogAttachment) ?? null;
  merged.catalogOpenWeights = preferIncoming(existing.catalogOpenWeights, incoming.catalogOpenWeights) ?? null;
  merged.catalogInputModalities = unionArray(existing.catalogInputModalities, incoming.catalogInputModalities);
  merged.catalogOutputModalities = unionArray(existing.catalogOutputModalities, incoming.catalogOutputModalities);
  merged.catalogContextLimit = preferIncoming(existing.catalogContextLimit, incoming.catalogContextLimit) ?? null;
  merged.catalogInputLimit = preferIncoming(existing.catalogInputLimit, incoming.catalogInputLimit) ?? null;
  merged.catalogOutputLimit = preferIncoming(existing.catalogOutputLimit, incoming.catalogOutputLimit) ?? null;
  merged.inputMicroUsdPerMillion = preferIncoming(existing.inputMicroUsdPerMillion, incoming.inputMicroUsdPerMillion) ?? null;
  merged.cachedInputMicroUsdPerMillion = preferIncoming(existing.cachedInputMicroUsdPerMillion, incoming.cachedInputMicroUsdPerMillion) ?? null;
  merged.cacheWrite5mMicroUsdPerMillion = preferIncoming(existing.cacheWrite5mMicroUsdPerMillion, incoming.cacheWrite5mMicroUsdPerMillion) ?? null;
  merged.cacheWrite1hMicroUsdPerMillion = preferIncoming(existing.cacheWrite1hMicroUsdPerMillion, incoming.cacheWrite1hMicroUsdPerMillion) ?? null;
  merged.outputMicroUsdPerMillion = preferIncoming(existing.outputMicroUsdPerMillion, incoming.outputMicroUsdPerMillion) ?? null;
  merged.imageRequestPrices = unionImagePrices(existing.imageRequestPrices, incoming.imageRequestPrices);
  merged.reasoningLevels = unionReasoningLevels(existing.reasoningLevels, incoming.reasoningLevels);
  merged.reasoningSupportedLevels = unionReasoningLevels(existing.reasoningSupportedLevels, incoming.reasoningSupportedLevels);
  merged.reasoningAllowedLevels = unionReasoningLevels(existing.reasoningAllowedLevels, incoming.reasoningAllowedLevels);
  merged.reasoningConfigurable = (existing.reasoningConfigurable ?? false) || (incoming.reasoningConfigurable ?? false);
  merged.reasoningManualFallback = (existing.reasoningManualFallback ?? false) || (incoming.reasoningManualFallback ?? false);
  merged.speedSupported = (existing.speedSupported ?? false) || (incoming.speedSupported ?? false);
  merged.speedTiers = unionArray(existing.speedTiers, incoming.speedTiers);
  merged.speedTier = existing.speedTier ?? incoming.speedTier ?? "standard";
  merged.speedConfigurable = (existing.speedConfigurable ?? false) || (incoming.speedConfigurable ?? false);
  return merged;
}

function fallbackModelSummary(id: string): ModelSummary {
  return {
    id,
    enabled: true,
    memberCount: 0,
    codexVisible: false,
    codexDisplayName: id,
    catalogProvider: null,
    catalogFamily: null,
    catalogName: null,
    catalogReleaseDate: null,
    catalogLastUpdated: null,
    catalogStatus: null,
    catalogReasoning: null,
    catalogReasoningMethod: null,
    catalogReasoningEffortLevels: [],
    catalogDefaultReasoningEffort: null,
    catalogToolCall: null,
    catalogStructuredOutput: null,
    catalogAttachment: null,
    catalogOpenWeights: null,
    catalogInputModalities: [],
    catalogOutputModalities: [],
    catalogContextLimit: null,
    catalogInputLimit: null,
    catalogOutputLimit: null,
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
    speedTiers: [],
    speedSupported: false,
    speedTier: "standard",
    speedConfigurable: false,
  };
}

/**
 * Operational views may filter the inventory by live availability. Model Rules
 * uses currentPoolModelSummaries instead, so configured models stay editable.
 */
export function operationalModelSummaries(runtime: RuntimeSnapshot): ModelSummary[] {
  return modelSummaries(runtime).filter((model) =>
    hasOperationalModelRoute(runtime, model.id),
  );
}

function hasOperationalModelRoute(runtime: RuntimeSnapshot, modelId: string) {
  const nowMs = Date.now();
  return runtime.sources.some((source) =>
    sourceHasOperationalModelRoute(source, runtime.gateway.routingOrder, modelId, nowMs),
  ) || runtime.accounts.some((account) =>
    accountHasOperationalModelRoute(account, runtime.gateway.routingOrder, modelId, nowMs),
  );
}

function sourceHasOperationalModelRoute(
  source: SourceSummary,
  routingOrder: readonly CandidateRuntimeSnapshot[] | undefined,
  modelId: string,
  nowMs: number,
) {
  if (!memberCanServeModel(source, modelId)) return false;

  // Keep this projection in lockstep with the runtime normalizer. In
  // particular, a sole native binding with an empty model list is the legacy
  // source-wide form and means all models in `source.models`.
  const bindings = effectiveSourceProtocolBindings(source);
  const matchingBindings = bindings.filter((binding) =>
    sourceBindingModels(source, bindings, binding).some((model) => sameModel(model, modelId)),
  );
  if (!matchingBindings.length) return false;
  return hasLiveRoute(
    routingOrder,
    "api_source",
    matchingBindings.map((binding) => sourceCandidateId(source.id, binding, source.wireApi)),
    modelId,
    nowMs,
  );
}

function sourceBindingModels(
  source: Pick<SourceSummary, "models">,
  bindings: readonly SourceProtocolBinding[],
  binding: SourceProtocolBinding,
): readonly string[] {
  return binding.modelIds.length
    || bindings.length !== 1
    || normalizedAdapter(binding) !== "native"
    ? binding.modelIds
    : source.models;
}

function accountHasOperationalModelRoute(
  account: AccountSummary,
  routingOrder: readonly CandidateRuntimeSnapshot[] | undefined,
  modelId: string,
  nowMs: number,
) {
  return account.proxyAvailable !== false
    && memberCanServeModel(account, modelId)
    && account.models.some((model) => sameModel(model, modelId))
    && hasLiveRoute(routingOrder, "oauth_account", [account.id], modelId, nowMs);
}

function memberCanServeModel(
  member: Pick<AccountSummary, "enabled" | "inPool" | "draining" | "secretAvailable" | "operationalStatus" | "allowedModels" | "excludedModels">,
  modelId: string,
) {
  return member.enabled
    && member.inPool
    && !member.draining
    && member.secretAvailable
    && member.operationalStatus === "rotation"
    && modelAllowedByMember(member, modelId);
}

function modelAllowedByMember(
  member: Pick<AccountSummary, "allowedModels" | "excludedModels">,
  modelId: string,
) {
  return !member.excludedModels.some((rule) => matchesModelRule(rule, modelId))
    && (!member.allowedModels.length
      || member.allowedModels.some((rule) => matchesModelRule(rule, modelId)));
}

function hasLiveRoute(
  routingOrder: readonly CandidateRuntimeSnapshot[] | undefined,
  kind: CandidateRuntimeSnapshot["kind"],
  candidateIds: readonly string[],
  modelId: string,
  nowMs: number,
) {
  // Older remote snapshots, and snapshots taken while the gateway is
  // stopped, do not include route-level state. Their member operationalStatus
  // remains the compatible source of truth; current snapshots additionally
  // remove a model while its exact route is cooling. An empty order therefore
  // means "not reported", rather than "no routes".
  if (routingOrder == null || routingOrder.length === 0) return true;
  const matchingCandidates = routingOrder.filter((candidate) =>
    candidate.kind === kind
      && candidateIds.includes(candidate.candidateId)
  );
  // A partial/older snapshot can contain other candidates but omit this one.
  // Do not turn that missing telemetry into a false availability failure; the
  // member's operational status above is the source of truth in that case.
  if (!matchingCandidates.length) return true;
  return matchingCandidates.some((candidate) =>
    !candidate.modelRetries?.some((retry) =>
      retry.retryAtMs > nowMs && matchesModelRule(retry.model, modelId),
    )
      // `available` is a volatile scheduler fact. A member already reported
      // as `rotation` is still a valid source of model rules when an older or
      // transient route snapshot says unavailable. A future whole-candidate
      // retry remains authoritative and hides every model on that route.
      && (candidate.available
        || candidate.nextRetryAtMs == null
        || candidate.nextRetryAtMs <= nowMs),
  );
}

function sourceCandidateId(
  sourceId: string,
  binding: SourceProtocolBinding,
  legacyProtocol: SourceProtocolBinding["wireApi"],
) {
  const adapter = binding.adapter ?? "native";
  if (adapter === "native" && binding.wireApi === legacyProtocol) return sourceId;
  const suffix = adapter === "native" ? binding.wireApi : adapter;
  return `${sourceId}::${suffix}`;
}

function matchesModelRule(rule: string, modelId: string) {
  const normalizedRule = rule.trim().toLowerCase();
  const normalizedModel = modelId.trim().toLowerCase();
  if (normalizedRule === "*") return true;
  return normalizedRule.endsWith("*")
    ? normalizedModel.startsWith(normalizedRule.slice(0, -1))
    : normalizedRule === normalizedModel;
}

function sameModel(left: string, right: string) {
  return left.trim().toLowerCase() === right.trim().toLowerCase();
}

export function groupModelSummaries(
  models: ModelSummary[],
  accounts: AccountSummary[],
) {
  const chatGptModelIds = new Set(
    accounts.flatMap((account) => account.models.map((model) => model.toLowerCase())),
  );
  return groupModels(
    models,
    {
      metadata: (model) => model,
      isNativeChatGpt: (model) => chatGptModelIds.has(model.id.toLowerCase()),
    },
  );
}

export function comparePoolMembers(
  left: PoolMember,
  right: PoolMember,
  order: Map<string, number>,
) {
  return (
    compareOperationalStatus(left.operationalStatus, right.operationalStatus) ||
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

export function compareStableText(left: string, right: string) {
  return left === right ? 0 : left < right ? -1 : 1;
}
