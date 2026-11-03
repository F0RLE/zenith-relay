import { modelIdKey, uniqueModelIds } from "../../../../src/src/features/relay/modelGroups";
import { memberModelIsEnabled, modelSelectionPayload } from "../../../../src/src/features/relay/components/poolMemberEditorModel";
import type { OwnerConnection } from "./model";

export interface OwnerMemberSettings {
  allowedModels: string[];
  excludedModels: string[];
  paused: boolean;
  purchaseCostMicroUsd: number | null;
}

/** Inventory stays intact: a disabled model is a rule, not a removed model. */
export function ownerMemberModels(member: OwnerConnection) {
  const modelIds = uniqueModelIds(member.models);
  const policy = member.modelPolicy;
  return {
    modelIds,
    enabledModels: modelIds.filter(model => memberModelIsEnabled(policy?.allowedModels ?? [], policy?.excludedModels ?? [], model)),
  };
}

export function ownerMemberModelPolicy(member: OwnerConnection, enabledModels: readonly string[]) {
  const { modelIds } = ownerMemberModels(member);
  const known = new Set(modelIds.map(modelIdKey));
  const selection = modelSelectionPayload(modelIds, enabledModels);
  // Temporarily absent models must not lose their saved exclusion.
  selection.excludedModels = uniqueModelIds([
    ...(member.modelPolicy?.excludedModels ?? []).filter(model => !model.includes("*") && !known.has(modelIdKey(model))),
    ...selection.excludedModels,
  ]);
  return selection;
}

/** Exact USD-to-microUSD conversion; empty means unknown, not a free account. */
export function parseMemberPurchaseCost(text: string): number | null | undefined {
  const value = text.trim().replace(",", ".");
  if (!value) return null;
  if (!/^\d+(?:\.\d{1,6})?$/.test(value)) return undefined;
  const [whole, fraction = ""] = value.split(".");
  const microUsd = Number(whole) * 1_000_000 + Number(fraction.padEnd(6, "0"));
  return Number.isSafeInteger(microUsd) && microUsd >= 0 && microUsd <= 1_000_000_000_000 ? microUsd : undefined;
}

export function validMemberSettings(settings: OwnerMemberSettings) {
  const cost = settings.purchaseCostMicroUsd;
  return typeof settings.paused === "boolean"
    && (cost === null || Number.isSafeInteger(cost) && cost >= 0 && cost <= 1_000_000_000_000)
    && [settings.allowedModels, settings.excludedModels].every(models => Array.isArray(models)
      && models.every(model => typeof model === "string" && model.trim().length > 0));
}
