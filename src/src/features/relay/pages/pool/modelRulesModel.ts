import type { DefaultServiceTier, ModelSummary } from "../../api/types";
import { modelIdKey } from "../../modelGroups";
import { normalizeReasoningEffort } from "../../poolFormatting";

export type ModelRuleGroup = {
  id: string;
  label: string;
  models: ModelSummary[];
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
export function reorderById<T extends { id: string }>(rows: readonly T[], sourceId: string, targetId: string) {
  if (sourceId === targetId) return null;
  const sourceIndex = rows.findIndex((modelRow) => modelRow.id === sourceId);
  const targetIndex = rows.findIndex((modelRow) => modelRow.id === targetId);
  if (sourceIndex < 0 || targetIndex < 0) return null;
  const reorderedRows = [...rows];
  const [moved] = reorderedRows.splice(sourceIndex, 1);
  reorderedRows.splice(targetIndex, 0, moved!);
  return reorderedRows;
}

/** Flatten groups after moving a complete provider block to another block. */
export function reorderModelGroups(groups: readonly ModelRuleGroup[], sourceId: string, targetId: string) {
  if (sourceId === targetId) return null;
  const sourceIndex = groups.findIndex((group) => group.id === sourceId);
  const targetIndex = groups.findIndex((group) => group.id === targetId);
  if (sourceIndex < 0 || targetIndex < 0) return null;
  const blocks = groups.map((group) => [...group.models]);
  const [moved] = blocks.splice(sourceIndex, 1);
  blocks.splice(targetIndex, 0, moved!);
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
    const modelId = model.id.trim();
    const key = modelIdKey(modelId);
    if (!modelId || included.has(key)) return;
    included.add(key);
    order.push(modelId);
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
  const outputs = (model.catalogOutputModalities ?? []).map((modality) => modality.toLowerCase());
  if (outputs.includes("image") && !outputs.includes("text")) return true;
  const family = model.catalogFamily?.toLowerCase() ?? "";
  const familyTokens = family.split(/[^a-z0-9]+/).filter(Boolean);
  if (familyTokens.includes("image") || familyTokens.includes("dalle")) return true;
  const modelId = model.id.toLowerCase();
  return modelId.startsWith("gpt-image") || modelId.startsWith("dall-e") || modelId.startsWith("dalle");
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
  const remainingPending: Record<string, boolean> = {};
  for (const [id, enabled] of Object.entries(pending)) {
    if (confirmed.get(id) === enabled) {
      changed = true;
      continue;
    }
    remainingPending[id] = enabled;
  }
  return changed ? remainingPending : pending;
}

/** Remove one pending value only when it is still the failed attempt. */
export function clearPendingModelEnabled(
  pending: Readonly<Record<string, boolean>>,
  id: string,
  enabled: boolean,
) {
  if (pending[id] !== enabled) return pending;
  const remainingPending = { ...pending };
  delete remainingPending[id];
  return remainingPending;
}
