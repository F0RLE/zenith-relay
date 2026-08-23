import type {
  MessagesReasoningMode,
  CacheWriteTtl,
  SourceAdapter,
  SourceProtocolBinding,
  SourceSummary,
  SourceWireApi,
} from "./api/types";

export const sourceWireApis = [
  "responses",
  "messages",
  "chat_completions",
] as const satisfies readonly SourceWireApi[];

const supportedMessagesReasoningModes: readonly MessagesReasoningMode[] = [
  "disabled",
  "budget",
  "adaptive",
];

function isSourceWireApi(value: string): value is SourceWireApi {
  return sourceWireApis.includes(value as SourceWireApi);
}

function normalizedCacheWriteTtl(value: SourceProtocolBinding): CacheWriteTtl {
  return value.cacheWriteTtl === "5m" || value.cacheWriteTtl === "1h"
    ? value.cacheWriteTtl
    : "provider";
}

export function normalizedAdapter(binding: SourceProtocolBinding): SourceAdapter {
  return (binding.adapter === "responses_to_messages" || binding.adapter === "responses_to_gemini")
    && binding.wireApi === "responses"
    ? binding.adapter
    : "native";
}

export function normalizedReasoningMode(
  binding: SourceProtocolBinding,
  adapter = normalizedAdapter(binding),
): MessagesReasoningMode {
  return adapter === "responses_to_messages"
    && supportedMessagesReasoningModes.includes(binding.reasoningMode ?? "disabled")
    ? binding.reasoningMode ?? "disabled"
    : "disabled";
}

export function normalizedModelIds(modelIds: readonly string[], availableModels: readonly string[]) {
  const knownModels = new Map(
    availableModels.map((model) => [model.toLowerCase(), model] as const),
  );
  const seen = new Set<string>();
  return modelIds.flatMap((model) => {
    const normalized = model.trim().toLowerCase();
    const known = knownModels.get(normalized);
    if (!known || seen.has(normalized)) return [];
    seen.add(normalized);
    return [known];
  });
}

export function normalizedBindings(
  bindings: readonly SourceProtocolBinding[],
  availableModels: readonly string[],
): SourceProtocolBinding[] {
  const seen = new Set<string>();
  return bindings.flatMap((binding) => {
    const adapter = normalizedAdapter(binding);
    const routeKey = `${binding.wireApi}:${adapter}`;
    if (!isSourceWireApi(binding.wireApi) || seen.has(routeKey)) return [];
    seen.add(routeKey);
    const modelIds = binding.modelIds.length
      ? normalizedModelIds(binding.modelIds, availableModels)
      : [];
    return [{
      wireApi: binding.wireApi,
      modelIds,
      adapter,
      reasoningMode: normalizedReasoningMode(binding, adapter),
      cacheWriteTtl: normalizedCacheWriteTtl(binding),
    }];
  });
}

type ProtocolBindingSource = Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">;

/**
 * Legacy source records keep a single `wireApi`. Treat them as one virtual
 * binding in the UI so an edit never has to guess a protocol from a provider
 * name or silently widen the source's surface.
 */
export function effectiveSourceProtocolBindings(
  source: ProtocolBindingSource,
): SourceProtocolBinding[] {
  const configured = source.protocolBindings?.length
    ? normalizedBindings(source.protocolBindings, source.models)
    : [];
  return configured.length
    ? configured
    : [{ wireApi: source.wireApi, modelIds: [...source.models] }];
}

function sourceBindingModels(
  source: ProtocolBindingSource,
  bindings: readonly SourceProtocolBinding[],
  binding: SourceProtocolBinding,
) {
  return binding.modelIds.length || bindings.length !== 1 || normalizedAdapter(binding) !== "native"
    ? binding.modelIds
    : source.models;
}

/** Mirrors the runtime-only link from a confirmed native Messages route to
 * the Responses client. Explicit native Responses and Gemini assignments keep
 * ownership of overlapping models; an existing Messages bridge retains its
 * configured reasoning/cache policy while gaining unclaimed native models. */
export function runtimeSourceProtocolBindings(
  source: ProtocolBindingSource,
): SourceProtocolBinding[] {
  const bindings = effectiveSourceProtocolBindings(source);
  const claimedByOtherResponsesRoutes = new Set(
    bindings.flatMap((binding) => (
      binding.wireApi === "responses" && normalizedAdapter(binding) !== "responses_to_messages"
        ? sourceBindingModels(source, bindings, binding).map((model) => model.toLowerCase())
        : []
    )),
  );
  const linkedModels = normalizedModelIds(
    bindings.flatMap((binding) => (
      binding.wireApi === "messages" && normalizedAdapter(binding) === "native"
        ? sourceBindingModels(source, bindings, binding)
        : []
    )).filter((model) => !claimedByOtherResponsesRoutes.has(model.toLowerCase())),
    source.models,
  );
  if (!linkedModels.length) return bindings;

  const bridge = bindings.find((binding) => (
    binding.wireApi === "responses" && normalizedAdapter(binding) === "responses_to_messages"
  ));
  if (bridge) {
    return bindings.map((binding) => binding === bridge ? {
      ...binding,
      modelIds: normalizedModelIds([...binding.modelIds, ...linkedModels], source.models),
    } : binding);
  }
  return [...bindings, {
    wireApi: "responses",
    adapter: "responses_to_messages",
    reasoningMode: "disabled",
    cacheWriteTtl: "provider",
    modelIds: linkedModels,
  }];
}

/**
 * Mirrors the runtime's source capability calculation for one client
 * protocol. A sole empty binding retains the legacy source-wide catalog;
 * empty bindings in a multi-route source remain intentionally unconfirmed.
 */
export function sourceModelsForWireApi(
  source: ProtocolBindingSource,
  wireApi: SourceWireApi,
) {
  const bindings = runtimeSourceProtocolBindings(source);
  const seen = new Set<string>();
  return bindings.flatMap((binding) => {
    if (binding.wireApi !== wireApi) return [];
    return sourceBindingModels(source, bindings, binding).filter((model) => {
      const normalized = model.toLowerCase();
      if (seen.has(normalized)) return false;
      seen.add(normalized);
      return true;
    });
  });
}

export function sourceSupportsWireApi(
  source: ProtocolBindingSource,
  wireApi: SourceWireApi,
) {
  return sourceModelsForWireApi(source, wireApi).length > 0;
}

/**
 * A direct ChatGPT profile bypasses Relay entirely. It can use a real
 * Responses endpoint, but it cannot execute a Relay-owned bridge.
 */
export function sourceSupportsNativeResponses(source: ProtocolBindingSource) {
  const bindings = effectiveSourceProtocolBindings(source);
  return bindings.some(
    (binding) =>
      binding.wireApi === "responses"
      && normalizedAdapter(binding) === "native"
      && sourceBindingModels(source, bindings, binding).length > 0,
  );
}
