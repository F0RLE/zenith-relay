import type { ModelSummary } from "../../api/types";

export type ModelRuleGroup = {
  id: string;
  label: string;
  items: ModelSummary[];
};

/** Build the render-affecting identity used to reset an optimistic order. */
export function modelSignature(models: ModelSummary[]) {
  return models.map((model) => [
    model.id,
    model.enabled,
    model.speedSupported,
    model.speedTiers?.join(","),
    model.speedTier,
    model.speedConfigurable,
    model.codexVisible,
    model.codexDisplayName,
    model.catalogProvider,
    model.catalogFamily,
    model.catalogName,
    model.catalogReleaseDate,
    model.catalogLastUpdated,
    model.catalogStatus,
    model.catalogReasoningMethod,
    model.catalogReasoning,
    model.catalogReasoningEffortLevels?.join(","),
    model.inputMicroUsdPerMillion,
    model.cachedInputMicroUsdPerMillion,
    model.cacheWrite5mMicroUsdPerMillion,
    model.cacheWrite1hMicroUsdPerMillion,
    model.outputMicroUsdPerMillion,
    model.customPrice,
    model.reasoningLevels?.join(","),
    model.reasoningSupportedLevels?.join(","),
    model.reasoningAllowedLevels?.join(","),
    model.reasoningConfigurable,
    model.reasoningManualFallback,
  ].join(":" )).join("\u0000");
}

/** Reorder two rows without mutating the catalog delivered by the runtime. */
export function reorderById<T extends { id: string }>(items: readonly T[], sourceId: string, targetId: string) {
  if (sourceId === targetId) return null;
  const source = items.findIndex((item) => item.id === sourceId);
  const target = items.findIndex((item) => item.id === targetId);
  if (source < 0 || target < 0) return null;
  const next = [...items];
  const [moved] = next.splice(source, 1);
  next.splice(target, 0, moved!);
  return next;
}

/** Flatten groups after moving a complete provider block to another block. */
export function reorderModelGroups(groups: readonly ModelRuleGroup[], sourceId: string, targetId: string) {
  if (sourceId === targetId) return null;
  const source = groups.findIndex((group) => group.id === sourceId);
  const target = groups.findIndex((group) => group.id === targetId);
  if (source < 0 || target < 0) return null;
  const blocks = groups.map((group) => [...group.items]);
  const [moved] = blocks.splice(source, 1);
  blocks.splice(target, 0, moved!);
  return blocks.flat();
}

/** Keep every current pool model in the persisted order. */
export function completeModelDisplayOrder(
  reordered: readonly ModelSummary[],
  catalog: readonly ModelSummary[],
) {
  const included = new Set<string>();
  const order: string[] = [];
  const add = (model: ModelSummary) => {
    const id = model.id.trim();
    const key = id.toLowerCase();
    if (!id || included.has(key)) return;
    included.add(key);
    order.push(id);
  };
  reordered.forEach(add);
  catalog.forEach(add);
  return order;
}

export function supportedReasoningLevels(model: Pick<ModelSummary, "reasoningSupportedLevels" | "reasoningLevels" | "reasoningManualFallback">) {
  const declaredLevels = model.reasoningSupportedLevels?.length
    ? model.reasoningSupportedLevels
    : model.reasoningLevels ?? [];
  const levels = declaredLevels;
  const seen = new Set<string>();
  return levels
    .map((level) => level.trim().toLowerCase())
    .filter((level) => Boolean(level) && !seen.has(level) && seen.add(level));
}

/** Keep selected values in provider order and remove stale policy values. */
export function normalizeReasoningSelection(supported: readonly string[], selected: readonly string[]) {
  const selectedSet = new Set(selected.map((level) => level.trim().toLowerCase()));
  return supported.filter((level) => selectedSet.has(level));
}
