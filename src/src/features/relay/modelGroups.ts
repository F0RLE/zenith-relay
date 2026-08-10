type KnownModelProviderGroup = "chatgpt" | "openai" | "anthropic" | "other";

export type ModelProviderGroup = KnownModelProviderGroup | `provider-${string}`;

type ModelGroup<T> = {
  id: ModelProviderGroup;
  label: string;
  items: T[];
};

const knownGroupOrder: KnownModelProviderGroup[] = ["chatgpt", "openai", "anthropic"];
const otherGroup = "other" as const;
const dynamicGroupPrefix = "provider-";
const knownGroupRank = new Map<string, number>(knownGroupOrder.map((group, index) => [group, index]));

function modelLeaf(model: string) {
  const id = model.trim().toLowerCase();
  return id.slice(id.lastIndexOf("/") + 1);
}

function isOpenAiModel(model: string) {
  return /^(gpt-|codex-|o\d|text-|dall-e)/.test(model);
}

function dynamicModelFamily(model: string) {
  const id = modelLeaf(model);
  const family = id.match(/^[a-z]+(?:\d+(?=[-._]|$))?/i)?.[0]?.replace(/\d+$/, "").toLowerCase();
  return family || null;
}

function dynamicGroupLabel(family: string) {
  return family.length <= 3
    ? family.toUpperCase()
    : `${family[0]!.toUpperCase()}${family.slice(1)}`;
}

export function modelProviderGroupLabel(group: ModelProviderGroup) {
  return group.startsWith(dynamicGroupPrefix)
    ? dynamicGroupLabel(group.slice(dynamicGroupPrefix.length))
    : group;
}

export function modelProviderGroup(model: string, nativeChatGpt = false): ModelProviderGroup {
  const id = modelLeaf(model);
  if (nativeChatGpt && isOpenAiModel(id)) return "chatgpt";
  if (id.startsWith("claude-")) return "anthropic";
  if (isOpenAiModel(id)) return "openai";
  const family = dynamicModelFamily(id);
  return family ? `provider-${family}` : otherGroup;
}

function compareModelIdsForLauncher(
  left: string,
  right: string,
  leftNativeChatGpt: boolean,
  rightNativeChatGpt: boolean,
) {
  const leftGroup = modelProviderGroup(left, leftNativeChatGpt);
  const rightGroup = modelProviderGroup(right, rightNativeChatGpt);
  const leftRank = knownGroupRank.get(leftGroup) ?? (leftGroup === otherGroup ? Number.MAX_SAFE_INTEGER : knownGroupOrder.length);
  const rightRank = knownGroupRank.get(rightGroup) ?? (rightGroup === otherGroup ? Number.MAX_SAFE_INTEGER : knownGroupOrder.length);
  return leftRank - rightRank;
}

/// Launcher-only presentation ordering. Sources stay provider-agnostic and
/// unknown IDs retain their first source response position. A caller may place
/// models exposed by a native ChatGPT account in their own first group.
export function sortModelsForLauncher<T>(
  items: T[],
  model: (item: T) => string,
  isNativeChatGpt: (item: T) => boolean = () => false,
) {
  return items
    .map((item, sourceOrder) => ({ item, sourceOrder }))
    .sort((left, right) => (
      compareModelIdsForLauncher(
        model(left.item),
        model(right.item),
        isNativeChatGpt(left.item),
        isNativeChatGpt(right.item),
      ) || left.sourceOrder - right.sourceOrder
    ))
    .map(({ item }) => item);
}

export function sortModelIdsForLauncher(models: string[]) {
  return sortModelsForLauncher(models, (model) => model);
}

export function groupModels<T>(
  items: T[],
  model: (item: T) => string,
  isNativeChatGpt: (item: T) => boolean = () => false,
) {
  const groups = new Map<ModelProviderGroup, T[]>();
  for (const item of sortModelsForLauncher(items, model, isNativeChatGpt)) {
    const group = modelProviderGroup(model(item), isNativeChatGpt(item));
    const values = groups.get(group);
    if (values) values.push(item);
    else groups.set(group, [item]);
  }
  const orderedGroups: ModelProviderGroup[] = [
    ...knownGroupOrder.filter((id) => groups.has(id)),
    ...[...groups.keys()].filter((id) => id.startsWith(dynamicGroupPrefix)),
    ...(groups.has(otherGroup) ? [otherGroup] : []),
  ];
  return orderedGroups.map((id): ModelGroup<T> => ({
    id,
    label: modelProviderGroupLabel(id),
    items: groups.get(id)!,
  }));
}

export function supportsCacheWritePricing(model: string) {
  return modelProviderGroup(model) === "anthropic";
}
