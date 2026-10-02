import type { DefaultServiceTier, ModelSummary } from "../../api/types";
import { modelIdKey } from "../../modelGroups";
import { normalizeReasoningEffort } from "../../poolFormatting";

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
    const key = modelIdKey(id);
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
    .map((level) => normalizeReasoningEffort(level))
    .filter((level) => Boolean(level) && !seen.has(level) && seen.add(level));
}

/** Image generation has no effort selector. Other models show one only when levels exist. */
export function modelShowsReasoningControl(model: Pick<ModelSummary, "id" | "catalogFamily" | "catalogOutputModalities" | "reasoningLevels" | "reasoningSupportedLevels" | "reasoningManualFallback">) {
  if (isImageGenerationModel(model)) return false;
  return (model.reasoningSupportedLevels?.length ?? 0) > 0
    || (model.reasoningLevels?.length ?? 0) > 0
    || model.reasoningManualFallback === true;
}

function isImageGenerationModel(model: Pick<ModelSummary, "id" | "catalogFamily" | "catalogOutputModalities">) {
  const outputs = (model.catalogOutputModalities ?? []).map((item) => item.toLowerCase());
  if (outputs.includes("image") && !outputs.includes("text")) return true;
  const family = model.catalogFamily?.toLowerCase() ?? "";
  const familyTokens = family.split(/[^a-z0-9]+/).filter(Boolean);
  if (familyTokens.includes("image") || familyTokens.includes("dalle")) return true;
  const id = model.id.toLowerCase();
  return id.startsWith("gpt-image") || id.startsWith("dall-e") || id.startsWith("dalle");
}

/** Keep selected values in provider order and remove stale policy values. */
export function normalizeReasoningSelection(supported: readonly string[], selected: readonly string[]) {
  const selectedSet = new Set(selected.map((level) => normalizeReasoningEffort(level)));
  return supported.filter((level) => selectedSet.has(level));
}

const MODEL_SPEED_ORDER = ["standard", "fast", "ultrafast"] as const satisfies readonly DefaultServiceTier[];

/** Speed choices for one model. A configurable family always keeps all three modes. */
export function modelSpeedTiers(model: Pick<ModelSummary, "speedSupported" | "speedTiers">): DefaultServiceTier[] {
  const declared = new Set(model.speedTiers ?? []);
  const ordered = MODEL_SPEED_ORDER.filter((tier) => declared.has(tier));
  if (model.speedSupported && ordered.length <= 1) return [...MODEL_SPEED_ORDER];
  return ordered.length ? [...ordered] : ["standard"];
}

/** Shown switch state: the pending click wins until the runtime snapshot confirms it. */
export function pendingModelEnabled(
  pending: Readonly<Record<string, boolean>>,
  model: { id: string; enabled: boolean },
) {
  return pending[model.id] ?? model.enabled;
}

/** Drop pending switches once the runtime reports the same value. */
export function reconcilePendingModelEnabled(
  pending: Readonly<Record<string, boolean>>,
  models: readonly { id: string; enabled: boolean }[],
) {
  const confirmed = new Map(models.map((model) => [model.id, model.enabled]));
  let changed = false;
  const next: Record<string, boolean> = {};
  for (const [id, enabled] of Object.entries(pending)) {
    if (confirmed.get(id) === enabled) {
      changed = true;
      continue;
    }
    next[id] = enabled;
  }
  return changed ? next : pending;
}

/** Remove one pending value only when it is still the failed attempt. */
export function clearPendingModelEnabled(
  pending: Readonly<Record<string, boolean>>,
  id: string,
  enabled: boolean,
) {
  if (pending[id] !== enabled) return pending;
  const next = { ...pending };
  delete next[id];
  return next;
}
