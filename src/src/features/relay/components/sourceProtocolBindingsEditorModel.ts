import type {
  CacheWriteTtl,
  SourceAdapter,
  SourceProtocolBinding,
  SourceWireApi,
} from "../api/types";
import {
  normalizedAdapter,
  normalizedModelIds,
  upstreamWireApi,
} from "../sourceProtocolBindings";

type EditorContext = {
  bindings: readonly SourceProtocolBinding[];
  models: readonly string[];
  autoAssignModels: boolean;
};

function selectedModels(
  binding: SourceProtocolBinding,
  { bindings, models, autoAssignModels }: EditorContext,
) {
  return binding.modelIds.length || bindings.length > 1 || normalizedAdapter(binding) !== "native" || !autoAssignModels
    ? [...binding.modelIds]
    : [...models];
}

function routeBinding(
  bindings: readonly SourceProtocolBinding[],
  wireApi: SourceWireApi,
  adapter: SourceAdapter,
) {
  return bindings.find(
    (binding) => binding.wireApi === wireApi && normalizedAdapter(binding) === adapter,
  );
}

function addModelToRoute(
  sourceBindings: readonly SourceProtocolBinding[],
  target: SourceProtocolBinding,
  model: string,
  context: EditorContext,
) {
  const targetModels = selectedModels(target, context);
  return sourceBindings.map((binding) => binding === target
    ? { ...binding, modelIds: normalizedModelIds([...targetModels, model], context.models) }
    : binding);
}

export function updateNativeProtocol({
  bindings,
  models,
  autoAssignModels,
  wireApi,
  selected,
}: EditorContext & { wireApi: SourceWireApi; selected: boolean }) {
  const context = { bindings, models, autoAssignModels };
  if (!selected) {
    return bindings.filter((binding) => !(
      binding.wireApi === wireApi && normalizedAdapter(binding) === "native"
    ));
  }
  const existing = routeBinding(bindings, wireApi, "native");
  if (existing) {
    const nextModels = normalizedModelIds([
      ...selectedModels(existing, context),
      ...models,
    ], models);
    return bindings.map((binding) => binding === existing
      ? { ...binding, modelIds: nextModels }
      : binding);
  }
  return [
    ...bindings,
    {
      wireApi,
      // A secondary format starts unassigned. Its model cells become active
      // immediately, while the header remains off until a model is routed.
      modelIds: bindings.length ? [] : [...models],
      adapter: "native" as const,
      reasoningMode: "disabled" as const,
      cacheWriteTtl: "provider" as const,
    },
  ];
}

export function updateModelRoute({
  bindings,
  models,
  autoAssignModels,
  wireApi,
  adapter,
  model,
  selected,
}: EditorContext & {
  wireApi: SourceWireApi;
  adapter: SourceAdapter;
  model: string;
  selected: boolean;
}) {
  const context = { bindings, models, autoAssignModels };
  const target = routeBinding(bindings, wireApi, adapter);
  if (!target) return selected ? [...bindings, {
    wireApi, adapter, modelIds: [model], reasoningMode: adapter === "native" ? "disabled" as const : "adaptive" as const,
  }] : [...bindings];
  if (!selected) {
    const selectedIds = selectedModels(target, context);
    const nextModelIds = selectedIds.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase());
    if (!nextModelIds.length && bindings.length > 1) {
      return bindings.filter((binding) => binding !== target);
    }
    return bindings.map((binding) => binding === target
      ? {
        ...binding,
        // A single legacy binding may use an empty list as its source-wide
        // fallback. Keep the final route intact so the source remains routable.
        modelIds: nextModelIds.length || bindings.length > 1
          ? normalizedModelIds(nextModelIds, models)
          : selectedIds,
      }
      : binding);
  }
  return addModelToRoute(bindings, target, model, context);
}

export function updateBridgeModel({
  bindings,
  models,
  autoAssignModels,
  adapter,
  model,
  selected,
  cacheWriteTtl,
}: EditorContext & {
  adapter: "responses_to_messages" | "responses_to_gemini";
  model: string;
  selected: boolean;
  cacheWriteTtl: CacheWriteTtl;
}) {
  const context = { bindings, models, autoAssignModels };
  const target = routeBinding(bindings, "responses", adapter);
  const bridgeModels = target ? selectedModels(target, context) : [];
  if (!selected) {
    if (!target) return [...bindings];
    const nextModelIds = normalizedModelIds(
      bridgeModels.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
      models,
    );
    return nextModelIds.length
      ? bindings.map((binding) => binding === target
        ? { ...binding, modelIds: nextModelIds }
        : binding)
      : bindings.filter((binding) => binding !== target);
  }

  if (target) {
    return addModelToRoute(bindings, target, model, context);
  }

  const newBinding: SourceProtocolBinding = adapter === "responses_to_messages"
    ? {
      wireApi: "responses",
      adapter,
      reasoningMode: "adaptive",
      cacheWriteTtl,
      modelIds: [model],
    }
    : {
      wireApi: "responses",
      adapter,
      reasoningMode: "adaptive",
      modelIds: [model],
    };
  return [
    ...bindings,
    newBinding,
  ];
}

export function updateCacheWriteTtl(
  bindings: readonly SourceProtocolBinding[],
  cacheWriteTtl: CacheWriteTtl,
) {
  return bindings.map((binding) => (
    upstreamWireApi(binding) === "messages"
      ? { ...binding, cacheWriteTtl }
      : binding
  ));
}
