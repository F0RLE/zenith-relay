import type { ModelCatalogIdentity, ModelSummary, RuntimeSnapshot } from "./api/types";
export type { ModelCatalogIdentity } from "./api/types";

export type ModelGroup<T> = {
  id: string;
  provider: string;
  label: string;
  models: T[];
};

type GroupModelsOptions<T> = {
  metadata?: (model: T) => ModelCatalogIdentity | null | undefined;
  isNativeChatGpt?: (model: T) => boolean;
};

const OTHER_PROVIDER = "other";

/** Model ids compare trimmed and case-insensitively. Callers keep the original spelling. */
export function modelIdKey(modelId: string) {
  return modelId.trim().toLowerCase();
}

/** Catalog metadata takes precedence over operational model rows. */
export function memberModelCatalog(gateway: RuntimeSnapshot["gateway"] | undefined) {
  return new Map<string, ModelCatalogIdentity>([
    ...(gateway?.models ?? []).map((model): [string, ModelCatalogIdentity] => [modelIdKey(model.id), model]),
    ...Object.entries(gateway?.modelCatalog ?? {}).map(([modelId, identity]): [string, ModelCatalogIdentity] => [modelIdKey(modelId), identity]),
  ]);
}

/**
 * Group models by provider. The runtime snapshot already carries the
 * provider-block order from the backend; preserve it, including an explicit
 * manual order. Family and version ordering belongs to the shared catalog.
 */
export function groupModels<T>(
  models: readonly T[],
  options: GroupModelsOptions<T> = {},
): ModelGroup<T>[] {
  const groups = new Map<string, ModelGroup<T>>();
  for (const model of models) {
    const metadata = options.metadata?.(model);
    const provider = normalizeCatalogValue(metadata?.catalogProvider)
      ?? (options.isNativeChatGpt?.(model) ? "openai" : OTHER_PROVIDER);
    const key = provider;
    let group = groups.get(key);
    if (!group) {
      group = {
        id: `catalog-${encodeURIComponent(provider)}`,
        provider,
        label: provider === OTHER_PROVIDER ? "Other" : displayCatalogValue(provider),
        models: [],
      };
      groups.set(key, group);
    }
    group.models.push(model);
  }
  return [...groups.values()];
}

/** Deduplicate model IDs without applying a second presentation order. */
export function uniqueModelIds(models: readonly string[]) {
  const seenModelIds = new Set<string>();
  return models.filter((model) => {
    const key = modelIdKey(model);
    return Boolean(key) && !seenModelIds.has(key) && seenModelIds.add(key);
  });
}

/**
 * Put IDs known by the current snapshot in backend order. IDs found only in a
 * member or usage inventory follow that inventory's first-seen order.
 * Complete catalog positions include disabled, excluded, and rule-only IDs.
 */
export function orderModelIdsBySnapshot(
  models: readonly string[],
  summaries: readonly ModelSummary[],
  catalog: RuntimeSnapshot["gateway"]["modelCatalog"] = undefined,
) {
  const unique = uniqueModelIds(models);
  const byId = new Map(unique.map((model) => [modelIdKey(model), model]));
  const catalogIds = Object.entries(catalog ?? {})
    .filter(([, identity]) => Number.isSafeInteger(identity.catalogOrder) && identity.catalogOrder! >= 0)
    .sort(([, left], [, right]) => left.catalogOrder! - right.catalogOrder!)
    .map(([modelId]) => modelId);
  const ordered = uniqueModelIds([...catalogIds, ...summaries.map((model) => model.id)])
    .map((modelId) => byId.get(modelIdKey(modelId)))
    .filter((model): model is string => model !== undefined);
  const knownModelIds = new Set(ordered.map((model) => modelIdKey(model)));
  const unknownModels = unique.filter((model) => !knownModelIds.has(modelIdKey(model)));
  return [...ordered, ...unknownModels];
}

function normalizeCatalogValue(catalogValue: string | null | undefined) {
  const normalizedProvider = catalogValue?.trim().toLowerCase();
  if (!normalizedProvider) return null;
  if (normalizedProvider === "x-ai" || normalizedProvider === "x_ai") return "xai";
  return normalizedProvider;
}

function displayCatalogValue(providerId: string) {
  return providerId
    .split(/[-_]+/)
    .filter(Boolean)
    .map(displayCatalogPart)
    .join(" ");
}

function displayCatalogPart(part: string) {
  if (part === "openai") return "OpenAI";
  if (part === "xai") return "xAI";
  if (part === "zai") return "Z.ai";
  if (part === "deepseek") return "DeepSeek";
  if (part === "moonshotai") return "Moonshot AI";
  if (part === "minimax") return "MiniMax";
  if (part === "nvidia") return "NVIDIA";
  if (part === "stepfun") return "StepFun";
  if (part === "bytedance") return "ByteDance";
  if (part === "nousresearch") return "Nous Research";
  return `${part[0]!.toUpperCase()}${part.slice(1)}`;
}
