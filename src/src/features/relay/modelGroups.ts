import type { ModelSummary } from "./api/types";

export type ModelCatalogIdentity = Pick<
  ModelSummary,
  "catalogProvider" | "catalogFamily"
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

/**
 * Group models by company, never by the catalog's finer-grained families.
 * Item order is never changed here: the snapshot is the presentation-order
 * authority and old snapshots without metadata retain discovery order.
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
    const existing = groups.get(key);
    if (existing) {
      existing.items.push(item);
      continue;
    }
    groups.set(key, {
      id: `catalog-${encodeURIComponent(provider)}`,
      provider,
      label: provider === OTHER_PROVIDER ? "Other" : displayCatalogValue(provider),
      items: [item],
    });
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
) {
  const unique = uniqueModelIds(models);
  const byId = new Map(unique.map((model) => [model.toLowerCase(), model]));
  const ordered = summaries
    .map((model) => byId.get(model.id.toLowerCase()))
    .filter((model): model is string => Boolean(model));
  const known = new Set(ordered.map((model) => model.toLowerCase()));
  return [...ordered, ...unique.filter((model) => !known.has(model.toLowerCase()))];
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
  if (part === "gpt") return "GPT";
  return `${part[0]!.toUpperCase()}${part.slice(1)}`;
}
