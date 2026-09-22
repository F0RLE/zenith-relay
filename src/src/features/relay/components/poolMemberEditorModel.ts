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
  const allowed = new Set(member.allowedModels.map((model) => model.toLocaleLowerCase()));
  const excluded = new Set(member.excludedModels.map((model) => model.toLocaleLowerCase()));
  return {
    modelIds,
    enabledModels: modelIds.filter((model) =>
      (!allowed.size || allowed.has(model.toLocaleLowerCase()))
      && !excluded.has(model.toLocaleLowerCase()),
    ),
  };
}

export function modelSelectionPayload(modelIds: readonly string[], enabledModels: readonly string[]) {
  const enabled = new Set(enabledModels.map((model) => model.toLocaleLowerCase()));
  const allEnabled = modelIds.every((model) => enabled.has(model.toLocaleLowerCase()));
  return {
    allowedModels: allEnabled ? [] : modelIds.filter((model) => enabled.has(model.toLocaleLowerCase())),
    excludedModels: allEnabled ? [] : modelIds.filter((model) => !enabled.has(model.toLocaleLowerCase())),
  };
}
