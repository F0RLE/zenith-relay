import type { ModelSummary, RuntimeSnapshot } from "./api/types";

export type ModelCatalogIdentity = Pick<
  ModelSummary,
  "catalogProvider"
>;

export type ModelGroup<T> = {
  id: string;
  provider: string;
  label: string;
  items: T[];
};

type GroupModelsOptions<T> = {
  metadata?: (item: T) => ModelCatalogIdentity | null | undefined;
  isNativeChatGpt?: (item: T) => boolean;
};

const OTHER_PROVIDER = "other";

/** Model ids compare trimmed and case-insensitively. Callers keep the original spelling. */
export function modelIdKey(value: string) {
  return value.trim().toLowerCase();
}

/** Older servers expose metadata only on operational model rows. */
export function memberModelCatalog(gateway: RuntimeSnapshot["gateway"] | undefined) {
  return new Map<string, ModelCatalogIdentity>([
    ...(gateway?.models ?? []).map((model): [string, ModelCatalogIdentity] => [modelIdKey(model.id), model]),
    ...Object.entries(gateway?.modelCatalog ?? {}).map(([id, identity]): [string, ModelCatalogIdentity] => [modelIdKey(id), identity]),
  ]);
}

/**
 * Group models by provider. The runtime snapshot already carries the
 * provider-block order from the backend; preserve it, including an explicit
 * manual order, and keep the source order inside each block.
 */
export function groupModels<T>(
  items: readonly T[],
  options: GroupModelsOptions<T> = {},
): ModelGroup<T>[] {
  const groups = new Map<string, ModelGroup<T>>();
  for (const item of items) {
    const metadata = options.metadata?.(item);
    const provider = normalizeCatalogValue(
      options.isNativeChatGpt?.(item) ? "openai" : metadata?.catalogProvider,
    ) ?? OTHER_PROVIDER;
    const key = provider;
    let group = groups.get(key);
    if (!group) {
      group = {
        id: `catalog-${encodeURIComponent(provider)}`,
        provider,
        label: provider === OTHER_PROVIDER ? "Other" : displayCatalogValue(provider),
        items: [],
      };
      groups.set(key, group);
    }
    group.items.push(item);
  }
  return [...groups.values()];
}

/** Deduplicate model IDs without applying a second presentation order. */
export function uniqueModelIds(models: readonly string[]) {
  const seen = new Set<string>();
  return models.filter((model) => {
    const key = modelIdKey(model);
    return Boolean(key) && !seen.has(key) && seen.add(key);
  });
}

/**
 * Put IDs known by the current snapshot in backend order. IDs found only in a
 * member or usage inventory follow that inventory's first-seen order.
 */
export function orderModelIdsBySnapshot(
  models: readonly string[],
  summaries: readonly ModelSummary[],
) {
  const unique = uniqueModelIds(models);
  const byId = new Map(unique.map((model) => [modelIdKey(model), model]));
  const ordered = summaries
    .map((model) => byId.get(modelIdKey(model.id)))
    .filter((model): model is string => Boolean(model));
  const known = new Set(ordered.map((model) => modelIdKey(model)));
  const unknown = unique.filter((model) => !known.has(modelIdKey(model)));
  return [...ordered, ...unknown];
}

function normalizeCatalogValue(value: string | null | undefined) {
  const normalized = value?.trim().toLowerCase();
  if (!normalized) return null;
  if (normalized === "x-ai" || normalized === "x_ai") return "xai";
  return normalized;
}

function displayCatalogValue(value: string) {
  return value
    .split(/[-_]+/)
    .filter(Boolean)
    .map(displayCatalogPart)
    .join(" ");
}

function displayCatalogPart(part: string) {
  if (part === "openai") return "OpenAI";
  if (part === "xai") return "xAI";
  if (part === "zai") return "Z.ai";
  return `${part[0]!.toUpperCase()}${part.slice(1)}`;
}
