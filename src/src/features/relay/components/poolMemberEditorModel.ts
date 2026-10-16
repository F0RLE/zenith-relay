import type { PoolMember } from "../poolHelpers";
import { uniqueModelIds } from "../modelGroups";
import { sourcePriceModels } from "./sourcePriceEditorModel";

export type ModelSelection = {
  modelIds: string[];
  enabledModels: string[];
};

export function modelSelectionForMember(member: PoolMember): ModelSelection {
  const modelIds = member.kind === "source" ? sourcePriceModels(member) : uniqueModelIds([
    ...member.models,
    ...member.allowedModels,
    ...member.excludedModels,
  ]);
  return {
    modelIds,
    enabledModels: modelIds.filter((model) => memberModelIsEnabled(member.allowedModels, member.excludedModels, model)),
  };
}

/** A model is off only when a saved rule says so. A later exact model stays on. */
export function memberModelIsEnabled(allowedModels: readonly string[], excludedModels: readonly string[], model: string) {
  const key = model.toLocaleLowerCase();
  const allowed = allowedModels.map((modelRule) => modelRule.toLocaleLowerCase());
  const excluded = excludedModels.map((modelRule) => modelRule.toLocaleLowerCase());
  if (excluded.includes(key)) return false;
  if (!allowed.length || allowed.includes(key)) return true;
  return excluded.length > 0 && allowed.every((rule) => !rule.includes("*")) && excluded.every((rule) => !rule.includes("*"));
}

export function modelSelectionPayload(modelIds: readonly string[], enabledModels: readonly string[]) {
  const enabled = new Set(enabledModels.map((model) => model.toLocaleLowerCase()));
  const allEnabled = modelIds.every((model) => enabled.has(model.toLocaleLowerCase()));
  return {
    allowedModels: [] as string[],
    excludedModels: allEnabled ? [] : modelIds.filter((model) => !enabled.has(model.toLocaleLowerCase())),
  };
}
