import type { PoolMember } from "../poolHelpers";
import { apiSourcePriority, type ApiSourceRole } from "../routingOrder";

export type ModelSelection = {
  modelIds: string[];
  enabledModels: string[];
};

export function modelSelectionForMember(member: PoolMember): ModelSelection {
  const pricedModels = member.kind === "source"
    ? [...Object.keys(member.modelPriceOverrides ?? {}), ...Object.keys(member.detectedModelPrices ?? {})]
    : [];
  const modelIds = uniqueModels([
    ...pricedModels,
    ...member.allowedModels,
    ...member.excludedModels,
    ...member.models,
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

export function moveSourceOrder(order: readonly string[], sourceId: string, targetId: string, after = false) {
  if (sourceId === targetId) return [...order];
  const next = order.filter((id) => id !== sourceId);
  const targetIndex = next.indexOf(targetId);
  if (targetIndex < 0) return [...order];
  next.splice(targetIndex + (after ? 1 : 0), 0, sourceId);
  return next;
}

export function moveSourceBy(order: readonly string[], sourceId: string, offset: number) {
  const index = order.indexOf(sourceId);
  const target = order[index + offset];
  return target ? moveSourceOrder(order, sourceId, target, offset > 0) : [...order];
}

export function sourcePrioritiesForOrder(order: readonly string[], role: ApiSourceRole) {
  return Object.fromEntries(order.map((sourceId, index) => [
    sourceId,
    apiSourcePriority(role, index, order.length),
  ]));
}

function uniqueModels(models: readonly string[]) {
  return [...new Map(models.map((model) => [model.toLocaleLowerCase(), model])).values()];
}
