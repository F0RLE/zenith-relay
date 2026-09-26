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

/** Older servers expose metadata only on operational model rows. */
export function memberModelCatalog(gateway: RuntimeSnapshot["gateway"] | undefined) {
  return new Map<string, ModelCatalogIdentity>([
    ...(gateway?.models ?? []).map((model): [string, ModelCatalogIdentity] => [model.id.toLowerCase(), model]),
    ...Object.entries(gateway?.modelCatalog ?? {}).map(([id, identity]): [string, ModelCatalogIdentity] => [id.toLowerCase(), identity]),
  ]);
}

/**
 * Group models by provider. Within each provider, preserve the snapshot order;
 * catalog families do not create a second presentation order.
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
    const key = model.trim().toLowerCase();
    return Boolean(key) && !seen.has(key) && seen.add(key);
  });
}

/**
 * Put IDs known by the current snapshot in backend order. IDs found only in
 * usage history follow in their first-seen order.
 */
export function orderModelIdsBySnapshot(
  models: readonly string[],
  summaries: readonly ModelSummary[],
  options: { unknownOrder?: "first-seen" | "stable-id" } = {},
) {
  const unique = uniqueModelIds(models);
  const byId = new Map(unique.map((model) => [model.toLowerCase(), model]));
  const ordered = summaries
    .map((model) => byId.get(model.id.toLowerCase()))
    .filter((model): model is string => Boolean(model));
  const known = new Set(ordered.map((model) => model.toLowerCase()));
  const unknown = unique.filter((model) => !known.has(model.toLowerCase()));
  if (options.unknownOrder === "stable-id") {
    unknown.sort(compareModelIds);
  }
  return [...ordered, ...unknown];
}

function compareModelIds(left: string, right: string) {
  const leftKey = left.trim().toLowerCase();
  const rightKey = right.trim().toLowerCase();
  return leftKey < rightKey ? -1 : leftKey > rightKey ? 1 : left < right ? -1 : left > right ? 1 : 0;
}

function normalizeCatalogValue(value: string | null | undefined) {
  const normalized = value?.trim().toLowerCase();
  return normalized || null;
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
