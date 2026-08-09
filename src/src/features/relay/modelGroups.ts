export type ModelProviderGroup = "chatgpt" | "openai" | "anthropic" | "other";

const groupOrder: ModelProviderGroup[] = ["chatgpt", "openai", "anthropic", "other"];
const groupRank = new Map(groupOrder.map((group, index) => [group, index]));

function modelLeaf(model: string) {
  const id = model.trim().toLowerCase();
  return id.slice(id.lastIndexOf("/") + 1);
}

function isOpenAiModel(model: string) {
  return /^(gpt-|codex-|o\d|text-|dall-e)/.test(model);
}

export function modelProviderGroup(model: string, nativeChatGpt = false): ModelProviderGroup {
  const id = modelLeaf(model);
  if (nativeChatGpt && isOpenAiModel(id)) return "chatgpt";
  if (id.startsWith("claude-")) return "anthropic";
  if (isOpenAiModel(id)) return "openai";
  return "other";
}

function compareModelIdsForLauncher(
  left: string,
  right: string,
  leftNativeChatGpt: boolean,
  rightNativeChatGpt: boolean,
) {
  const leftGroup = groupRank.get(modelProviderGroup(left, leftNativeChatGpt)) ?? Number.MAX_SAFE_INTEGER;
  const rightGroup = groupRank.get(modelProviderGroup(right, rightNativeChatGpt)) ?? Number.MAX_SAFE_INTEGER;
  return leftGroup - rightGroup;
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
  return groupOrder.flatMap((id) => groups.has(id) ? [{ id, items: groups.get(id)! }] : []);
}

export function supportsCacheWritePricing(model: string) {
  return modelProviderGroup(model) === "anthropic";
}
