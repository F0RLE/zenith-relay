import type {
  CacheWriteTtl,
  SourceAdapter,
  SourceProtocolBinding,
  SourceSummary,
  SourceWireApi,
} from "./api/types";
import { modelIdKey, uniqueModelIds } from "./modelGroups";

const sourceWireApis = [
  "responses",
  "chat_completions",
  "messages",
  "gemini",
  "gemini_interactions",
] as const satisfies readonly SourceWireApi[];

function isSourceWireApi(wireApiValue: string): wireApiValue is SourceWireApi {
  return sourceWireApis.includes(wireApiValue as SourceWireApi);
}

function normalizedCacheWriteTtl(binding: SourceProtocolBinding): CacheWriteTtl {
  return binding.cacheWriteTtl === "5m" || binding.cacheWriteTtl === "1h"
    ? binding.cacheWriteTtl
    : "provider";
}

function adapterBetween(client: SourceWireApi, upstream: SourceWireApi): SourceAdapter | undefined {
  if (client !== upstream && (client === "gemini_interactions" || upstream === "gemini_interactions")) return undefined;
  return client === upstream ? "native" : `${client}_to_${upstream}` as SourceAdapter;
}

function upstreamWireApi(binding: SourceProtocolBinding): SourceWireApi {
  const adapter = binding.adapter ?? "native";
  return sourceWireApis.find((upstream) => adapterBetween(binding.wireApi, upstream) === adapter) ?? binding.wireApi;
}

function normalizedModelIds(modelIds: readonly string[], availableModels: readonly string[]) {
  const availableModelsById = new Map(
    availableModels.map((model) => [modelIdKey(model), model] as const),
  );
  const models = uniqueModelIds(modelIds);
  if (availableModels.length && models.some((model) => !availableModelsById.has(modelIdKey(model)))) return null;
  return models.map((model) => (availableModelsById.get(modelIdKey(model)) ?? model).trim());
}

type ProtocolBindingSource = Pick<SourceSummary, "wireApi" | "protocolBindings" | "models" | "resolvedProtocolBindings">;

/** Prefer Rust's resolved routes; only a sole, unresolved legacy native route
 * may expand an empty model list to the source catalog. */
export function effectiveSourceProtocolBindings(
  source: ProtocolBindingSource,
): SourceProtocolBinding[] {
  const models = uniqueModelIds(source.models.map((model) => model.trim()));
  const bindings: readonly SourceProtocolBinding[] = source.resolvedProtocolBindings ?? (source.protocolBindings?.length
    ? source.protocolBindings
    : [{ wireApi: source.wireApi, modelIds: models }]);
  const expandLegacyModels = source.resolvedProtocolBindings == null && bindings.length === 1;
  const seenRouteKeys = new Set<string>();
  const normalized: SourceProtocolBinding[] = [];
  for (const binding of bindings) {
    const adapter = binding.adapter ?? "native";
    const routeKey = `${binding.wireApi}:${adapter}`;
    if (!isSourceWireApi(binding.wireApi) || seenRouteKeys.has(routeKey)) return [];
    const upstream = sourceWireApis.find((upstream) => adapterBetween(binding.wireApi, upstream) === adapter);
    const cacheWriteTtl = normalizedCacheWriteTtl(binding);
    if (!upstream || (cacheWriteTtl !== "provider" && upstream !== "messages")) return [];
    let modelIds = normalizedModelIds(binding.modelIds, models);
    if (modelIds === null) return [];
    if (!modelIds.length && expandLegacyModels && adapter === "native") modelIds = [...models];
    seenRouteKeys.add(routeKey);
    normalized.push({
      wireApi: binding.wireApi,
      modelIds,
      adapter,
      reasoningMode: adapter === "native" ? "disabled" : "adaptive",
      cacheWriteTtl,
    });
  }
  return normalized;
}

/**
 * Cache writes are an upstream Messages capability. Include both native
 * Messages routes and Responses routes translated by the Messages adapter.
 */
export function sourceModelsWithCacheWritePricing(source: ProtocolBindingSource) {
  const bindings = effectiveSourceProtocolBindings(source);
  const seenModelIds = new Set<string>();
  return bindings.flatMap((binding) => {
    const messagesUpstream = upstreamWireApi(binding) === "messages";
    if (!messagesUpstream) return [];
    return binding.modelIds.filter((model) => {
      const normalizedModelId = modelIdKey(model);
      if (seenModelIds.has(normalizedModelId)) return false;
      seenModelIds.add(normalizedModelId);
      return true;
    });
  });
}

/**
 * A direct ChatGPT profile bypasses Relay entirely. It can use a real
 * Responses endpoint, but it cannot execute a Relay-owned bridge.
 */
export function sourceSupportsNativeProtocol(source: ProtocolBindingSource, protocol?: SourceWireApi) {
  const bindings = effectiveSourceProtocolBindings(source);
  return bindings.some(
    (binding) =>
      (protocol === undefined || binding.wireApi === protocol)
      && binding.adapter === "native"
      && binding.modelIds.length > 0,
  );
}

export function sourceSupportsNativeResponses(source: ProtocolBindingSource) {
  return sourceSupportsNativeProtocol(source, "responses");
}
