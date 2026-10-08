import type {
  AccountSummary,
  ModelSummary,
  RuntimeSnapshot,
} from "./api/types";
import { groupModels, modelIdKey } from "./modelGroups";
import { sortReasoningEfforts } from "./poolFormatting";

export function modelSummaries(runtime: RuntimeSnapshot): ModelSummary[] {
  // `gateway.models` is a derived projection and can lag a source/account
  // refresh. Merge it with the catalogs carried by the current members even
  // when the derived array is non-empty. Keep the gateway order (it already
  // contains the saved presentation order), then append newly observed IDs.
  const summaries = new Map<string, ModelSummary>();
  const order: string[] = [];
  const catalog = new Map(
    Object.entries(runtime.gateway.modelCatalog ?? {}).map(([modelId, identity]) => [modelIdKey(modelId), identity]),
  );
  const add = (modelId: string, incomingModelSummary?: ModelSummary) => {
    const normalizedModelId = modelIdKey(modelId);
    if (!normalizedModelId) return;
    if (!summaries.has(normalizedModelId)) order.push(normalizedModelId);
    const incomingSummary = incomingModelSummary
      ? normalizeModelSummary(incomingModelSummary)
      : fallbackModelSummary(modelId.trim());
    const identity = catalog.get(normalizedModelId);
    // A source/account model can arrive before the derived gateway row. Keep
    // its catalog provider so the pool view still groups it correctly.
    if (identity && !incomingSummary.catalogProvider) {
      incomingSummary.catalogProvider = identity.catalogProvider ?? null;
      incomingSummary.catalogFamily = identity.catalogFamily ?? null;
    }
    const existingSummary = summaries.get(normalizedModelId);
    summaries.set(normalizedModelId, existingSummary
      ? mergeModelSummary(existingSummary, incomingSummary)
      : incomingSummary);
  };

  for (const model of runtime.gateway.models ?? []) add(model.id, model);
  for (const modelId of runtime.gateway.visibleModelIds) add(modelId);
  for (const source of runtime.sources) {
    for (const modelId of source.models) add(modelId);
    // A partially migrated source can have the model only on its binding.
    for (const binding of source.protocolBindings ?? []) {
      for (const modelId of binding.modelIds) add(modelId);
    }
  }
  for (const account of runtime.accounts) {
    for (const modelId of account.models) add(modelId);
  }

  const memberCount = new Map<string, number>();
  for (const member of [...runtime.sources, ...runtime.accounts]) {
    const memberModelIds = new Set(member.models.map((modelId) => modelIdKey(modelId)).filter(Boolean));
    for (const modelId of memberModelIds) memberCount.set(modelId, (memberCount.get(modelId) ?? 0) + 1);
  }

  return order.map((modelId) => {
    const modelSummary = summaries.get(modelId)!;
    const count = memberCount.get(modelId);
    return count == null ? modelSummary : { ...modelSummary, memberCount: count };
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
    for (const modelId of new Set(ids.map((modelId) => modelIdKey(modelId)).filter(Boolean))) {
      memberCounts.set(modelId, (memberCounts.get(modelId) ?? 0) + 1);
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
    .filter((model) => memberCounts.has(modelIdKey(model.id)))
    .map((model) => ({ ...model, memberCount: memberCounts.get(modelIdKey(model.id))! }));
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

function mergeModelSummary(existingSummary: ModelSummary, incomingSummary: ModelSummary): ModelSummary {
  const merged = { ...existingSummary };
  const preferIncoming = <T>(existingValue: T | null | undefined, incomingValue: T | null | undefined) =>
    existingValue == null || existingValue === "" ? incomingValue : existingValue;
  const unionArray = <T>(existingValues: readonly T[] | undefined, incomingValues: readonly T[] | undefined) =>
    [...new Set([...(existingValues ?? []), ...(incomingValues ?? [])])];
  const unionReasoningLevels = (existingLevels: readonly string[] | undefined, incomingLevels: readonly string[] | undefined) =>
    sortReasoningEfforts([...(existingLevels ?? []), ...(incomingLevels ?? [])]);
  const unionImagePrices = (existingPrices: ModelSummary["imageRequestPrices"], incomingPrices: ModelSummary["imageRequestPrices"]) => {
    const prices = new Map<string, NonNullable<ModelSummary["imageRequestPrices"]>[number]>();
    for (const price of [...(existingPrices ?? []), ...(incomingPrices ?? [])]) {
      const key = `${price.operation}:${price.quality}:${price.size}:${price.microUsd}`;
      if (!prices.has(key)) prices.set(key, price);
    }
    return [...prices.values()];
  };
  const mergeProtocolRoutes = (
    existingRoutes: ModelSummary["protocolRoutes"],
    incomingRoutes: ModelSummary["protocolRoutes"],
  ): NonNullable<ModelSummary["protocolRoutes"]> => {
    const routes = new Map<string, NonNullable<ModelSummary["protocolRoutes"]>[number]>();
    for (const route of [...(existingRoutes ?? []), ...(incomingRoutes ?? [])]) {
      const key = `${route.clientWireApi}:${route.upstreamWireApi}`;
      const existingRoute = routes.get(key);
      if (!existingRoute) {
        routes.set(key, {
          ...route,
          features: { ...route.features },
          reasoningEfforts: [...route.reasoningEfforts],
        });
        continue;
      }
      routes.set(key, {
        ...existingRoute,
        features: { ...existingRoute.features, ...route.features },
        reasoningEfforts: unionReasoningLevels(existingRoute.reasoningEfforts, route.reasoningEfforts),
      });
    }
    return [...routes.values()];
  };

  merged.memberCount = Math.max(existingSummary.memberCount, incomingSummary.memberCount);
  merged.protocolRoutes = mergeProtocolRoutes(existingSummary.protocolRoutes, incomingSummary.protocolRoutes);
  merged.codexDisplayName = existingSummary.codexDisplayName === existingSummary.id
    ? incomingSummary.codexDisplayName
    : existingSummary.codexDisplayName;
  merged.catalogProvider = preferIncoming(existingSummary.catalogProvider, incomingSummary.catalogProvider) ?? null;
  merged.catalogSourceModelId = preferIncoming(existingSummary.catalogSourceModelId, incomingSummary.catalogSourceModelId) ?? null;
  merged.catalogCanonicalModelId = preferIncoming(existingSummary.catalogCanonicalModelId, incomingSummary.catalogCanonicalModelId) ?? null;
  merged.catalogFamily = preferIncoming(existingSummary.catalogFamily, incomingSummary.catalogFamily) ?? null;
  merged.catalogName = preferIncoming(existingSummary.catalogName, incomingSummary.catalogName) ?? null;
  merged.catalogReleaseDate = preferIncoming(existingSummary.catalogReleaseDate, incomingSummary.catalogReleaseDate) ?? null;
  merged.catalogLastUpdated = preferIncoming(existingSummary.catalogLastUpdated, incomingSummary.catalogLastUpdated) ?? null;
  merged.catalogStatus = preferIncoming(existingSummary.catalogStatus, incomingSummary.catalogStatus) ?? null;
  merged.catalogReasoning = preferIncoming(existingSummary.catalogReasoning, incomingSummary.catalogReasoning) ?? null;
  merged.catalogReasoningMethod = preferIncoming(existingSummary.catalogReasoningMethod, incomingSummary.catalogReasoningMethod) ?? null;
  merged.catalogReasoningEffortLevels = unionReasoningLevels(existingSummary.catalogReasoningEffortLevels, incomingSummary.catalogReasoningEffortLevels);
  merged.catalogDefaultReasoningEffort = preferIncoming(existingSummary.catalogDefaultReasoningEffort, incomingSummary.catalogDefaultReasoningEffort) ?? null;
  merged.catalogToolCall = preferIncoming(existingSummary.catalogToolCall, incomingSummary.catalogToolCall) ?? null;
  merged.catalogStructuredOutput = preferIncoming(existingSummary.catalogStructuredOutput, incomingSummary.catalogStructuredOutput) ?? null;
  merged.catalogAttachment = preferIncoming(existingSummary.catalogAttachment, incomingSummary.catalogAttachment) ?? null;
  merged.catalogOpenWeights = preferIncoming(existingSummary.catalogOpenWeights, incomingSummary.catalogOpenWeights) ?? null;
  merged.catalogInputModalities = unionArray(existingSummary.catalogInputModalities, incomingSummary.catalogInputModalities);
  merged.catalogOutputModalities = unionArray(existingSummary.catalogOutputModalities, incomingSummary.catalogOutputModalities);
  merged.catalogContextLimit = preferIncoming(existingSummary.catalogContextLimit, incomingSummary.catalogContextLimit) ?? null;
  merged.catalogInputLimit = preferIncoming(existingSummary.catalogInputLimit, incomingSummary.catalogInputLimit) ?? null;
  merged.catalogOutputLimit = preferIncoming(existingSummary.catalogOutputLimit, incomingSummary.catalogOutputLimit) ?? null;
  merged.inputMicroUsdPerMillion = preferIncoming(existingSummary.inputMicroUsdPerMillion, incomingSummary.inputMicroUsdPerMillion) ?? null;
  merged.cachedInputMicroUsdPerMillion = preferIncoming(existingSummary.cachedInputMicroUsdPerMillion, incomingSummary.cachedInputMicroUsdPerMillion) ?? null;
  merged.cacheWrite5mMicroUsdPerMillion = preferIncoming(existingSummary.cacheWrite5mMicroUsdPerMillion, incomingSummary.cacheWrite5mMicroUsdPerMillion) ?? null;
  merged.cacheWrite1hMicroUsdPerMillion = preferIncoming(existingSummary.cacheWrite1hMicroUsdPerMillion, incomingSummary.cacheWrite1hMicroUsdPerMillion) ?? null;
  merged.outputMicroUsdPerMillion = preferIncoming(existingSummary.outputMicroUsdPerMillion, incomingSummary.outputMicroUsdPerMillion) ?? null;
  merged.imageRequestPrices = unionImagePrices(existingSummary.imageRequestPrices, incomingSummary.imageRequestPrices);
  merged.reasoningLevels = unionReasoningLevels(existingSummary.reasoningLevels, incomingSummary.reasoningLevels);
  merged.reasoningSupportedLevels = unionReasoningLevels(existingSummary.reasoningSupportedLevels, incomingSummary.reasoningSupportedLevels);
  merged.reasoningAllowedLevels = unionReasoningLevels(existingSummary.reasoningAllowedLevels, incomingSummary.reasoningAllowedLevels);
  merged.reasoningConfigurable = (existingSummary.reasoningConfigurable ?? false) || (incomingSummary.reasoningConfigurable ?? false);
  merged.reasoningManualFallback = (existingSummary.reasoningManualFallback ?? false) || (incomingSummary.reasoningManualFallback ?? false);
  merged.speedSupported = (existingSummary.speedSupported ?? false) || (incomingSummary.speedSupported ?? false);
  merged.speedTiers = unionArray(existingSummary.speedTiers, incomingSummary.speedTiers);
  merged.speedTier = existingSummary.speedTier ?? incomingSummary.speedTier ?? "standard";
  merged.speedConfigurable = (existingSummary.speedConfigurable ?? false) || (incomingSummary.speedConfigurable ?? false);
  return merged;
}

function fallbackModelSummary(modelId: string): ModelSummary {
  return {
    id: modelId,
    enabled: true,
    memberCount: 0,
    codexVisible: false,
    codexDisplayName: modelId,
    catalogProvider: null,
    catalogSourceModelId: null,
    catalogCanonicalModelId: null,
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

export function groupModelSummaries(
  models: ModelSummary[],
  accounts: AccountSummary[],
) {
  const chatGptModelIds = new Set(
    accounts.flatMap((account) => account.models.map((model) => modelIdKey(model))),
  );
  return groupModels(
    models,
    {
      metadata: (model) => model,
      isNativeChatGpt: (model) => chatGptModelIds.has(modelIdKey(model.id)),
    },
  );
}
