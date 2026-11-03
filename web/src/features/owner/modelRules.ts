import { modelIdKey } from "../../../../src/src/features/relay/modelGroups";
import { parseEditableModelPrice } from "../../../../src/src/features/relay/modelPricing";
import { selectedModelVariant } from "../models/model";
import type { CustomerModel, ModelRateComponent, ModelRates } from "../models/model";
import type { OwnerConnection } from "./model";
import { catalogGroups } from "../models/catalogOrder";

/** Server-wide policy, separate from a member's allowlist and current route health. */
export interface OwnerModelRule {
  enabled?: boolean;
  rates?: ModelRates;
  variantRates?: Record<string, ModelRates>;
}
export type OwnerModelRules = Record<string, OwnerModelRule>;

export interface OwnerCatalogSettings {
  rules: OwnerModelRules;
  /** Complete flattened provider blocks; positions include disabled models. */
  modelDisplayOrder: string[];
}

/** One preview projection for admin and customer catalogs. Server publication remains authoritative. */
export function projectOwnerCatalog(models: readonly CustomerModel[], settings: OwnerCatalogSettings) {
  return catalogGroups(models, settings.modelDisplayOrder).flatMap(group => group.models)
    .filter(model => settings.rules[model.id]?.enabled !== false)
    .map(model => applyOwnerModelRule(model, settings.rules[model.id]));
}

export function poolRuleModels(models: readonly CustomerModel[], connections: readonly OwnerConnection[]) {
  const inventory = new Set(connections.filter(member => member.inPool && !member.trashed).flatMap(member => member.models.map(modelIdKey)));
  const seen = new Set<string>();
  return models.filter(model => {
    const id = modelIdKey(model.id);
    if (!inventory.has(id) || seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}

/** Reuse Relay's price precision and bounds; blank is unpublished, never free. */
export function parseModelRate(text: string): bigint | null | undefined {
  if (!text.trim()) return null;
  const microUsd = parseEditableModelPrice(text);
  return microUsd === null ? undefined : BigInt(microUsd);
}

export function modelRateInput(value: bigint | null | undefined) {
  if (value == null || value < 0n) return "";
  const whole = value / 1_000_000n;
  const fraction = (value % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "");
  return `${whole}${fraction ? `.${fraction}` : ""}`;
}

export function setOwnerModelRate(rules: OwnerModelRules, model: CustomerModel, variantId: string | undefined, component: ModelRateComponent, value: bigint | null): OwnerModelRules {
  if (value !== null && (value < 0n || parseModelRate(modelRateInput(value)) !== value)) return rules;
  if (variantId !== undefined && !model.pricingVariants?.some(variant => variant.id === variantId)) return rules;
  const rule = rules[model.id] ?? {};
  const patch = { [component]: value };
  const isDefault = !variantId || selectedModelVariant(model)?.id === variantId;
  return { ...rules, [model.id]: {
    ...rule,
    ...(isDefault ? { rates: { ...rule.rates, ...patch } } : {}),
    ...(variantId ? { variantRates: { ...rule.variantRates, [variantId]: { ...rule.variantRates?.[variantId], ...patch } } } : {}),
  } };
}

/** Preview projection only: do not recompute official comparisons or billing. */
export function applyOwnerModelRule(model: CustomerModel, rule: OwnerModelRule | undefined): CustomerModel {
  if (!rule?.rates && !rule?.variantRates) return model;
  const base = rule.rates ? { ...model, rates: { ...model.rates, ...rule.rates } } : { ...model };
  if (rule.rates) delete base.comparison;
  const defaultId = selectedModelVariant(model)?.id;
  if (model.pricingVariants) base.pricingVariants = model.pricingVariants.map(variant => {
    const defaults = variant.id === defaultId ? rule.rates : undefined;
    const override = rule.variantRates?.[variant.id];
    if (!defaults && !override) return variant;
    const rates = { ...defaults, ...override };
    const updated = { ...variant, rates: { ...variant.rates, ...rates } };
    delete updated.comparison;
    return updated;
  });
  return base;
}
