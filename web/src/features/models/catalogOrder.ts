import { groupModels, modelIdKey } from "../../../../src/src/features/relay/modelGroups";
import { reorderById } from "../../../../src/src/features/relay/pages/pool/modelRulesModel";
import type { CustomerModel } from "./model";

/** Reuse Relay's company identity; never infer a provider from a model name. */
export function catalogGroups(models: readonly CustomerModel[], displayOrder: readonly string[] = []) {
  const remaining = new Map<string, CustomerModel>();
  for (const model of models) {
    const key = modelIdKey(model.id);
    if (key && !remaining.has(key)) remaining.set(key, model);
  }
  const ordered: CustomerModel[] = [];
  for (const id of displayOrder) {
    const key = modelIdKey(id);
    const model = remaining.get(key);
    if (!model) continue;
    ordered.push(model);
    remaining.delete(key);
  }
  ordered.push(...remaining.values());
  return groupModels(ordered, { metadata: model => ({ catalogProvider: model.catalogProvider ?? model.publisher ?? null }) });
}

export type CatalogMove = { kind: "group" | "model"; source: string; target: string };

/** Persist complete provider blocks, including disabled models. A drag cannot change company. */
export function moveCatalogItem(models: readonly CustomerModel[], displayOrder: readonly string[], move: CatalogMove): string[] | null {
  const groups = catalogGroups(models, displayOrder);
  if (move.kind === "group") {
    return reorderById(groups, move.source, move.target)?.flatMap(group => group.models.map(model => model.id)) ?? null;
  }
  const group = groups.find(group => group.models.some(model => model.id === move.source));
  if (!group) return null;
  const reordered = reorderById(group.models, move.source, move.target);
  if (!reordered) return null;
  return groups.flatMap(item => (item.id === group.id ? reordered : item.models).map(model => model.id));
}
