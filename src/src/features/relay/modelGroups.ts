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

type SemanticModelFamily = "openai" | "anthropic" | "gemini" | "grok" | "zai";

type SemanticModelSortKey = {
  familyRank: number;
  imageRank: number;
  tierRank: number;
  versionRank: number[];
  modifierRank: number;
  previewRank: number;
  id: string;
};

function modelLeaf(model: string) {
  const id = model.trim().toLowerCase();
  return id.slice(id.lastIndexOf("/") + 1);
}

function isOpenAiModel(model: string) {
  return /^(gpt-|codex-|o\d|text-|dall-e)/.test(model);
}

function semanticModelFamily(model: string): SemanticModelFamily | null {
  if (isOpenAiModel(model)) return "openai";
  if (model.startsWith("claude-")) return "anthropic";
  if (model.startsWith("gemini-")) return "gemini";
  if (model.startsWith("grok-")) return "grok";
  if (model.startsWith("glm-")) return "zai";
  return null;
}

function modelHasTerm(model: string, term: string) {
  return model.split(/[^a-z0-9]+/).includes(term);
}

function modelFamilyRank(family: SemanticModelFamily) {
  return ({ openai: 0, anthropic: 1, gemini: 2, grok: 3, zai: 4 } as const)[family];
}

function modelTierRank(family: SemanticModelFamily, model: string, isImage: boolean) {
  if (family === "anthropic") {
    if (modelHasTerm(model, "fable")) return 0;
    if (modelHasTerm(model, "opus")) return 1;
    if (modelHasTerm(model, "sonnet")) return 2;
    if (modelHasTerm(model, "haiku")) return 3;
    return 80;
  }
  if ((family === "gemini" || family === "openai") && isImage) return 90;
  if (family === "gemini") {
    if (modelHasTerm(model, "pro")) return 0;
    if (modelHasTerm(model, "lite")) return 2;
    if (modelHasTerm(model, "flash")) return 1;
    return 80;
  }
  if (family === "openai") {
    if (modelHasTerm(model, "mini") || modelHasTerm(model, "compact")) return 10;
    if (modelHasTerm(model, "spark")) return 20;
    return 0;
  }
  if (family === "grok") return modelHasTerm(model, "build") ? 10 : 0;
  if (family === "zai") {
    return modelHasTerm(model, "air") || modelHasTerm(model, "flash") || modelHasTerm(model, "lite") ? 10 : 0;
  }
  return isImage ? 9 : 0;
}

function modelModifierRank(family: SemanticModelFamily, model: string) {
  if (family === "openai") {
    if (model.endsWith("-sol")) return 1;
    if (model.endsWith("-terra")) return 2;
    if (model.endsWith("-luna")) return 3;
    return 8;
  }
  if (family === "gemini") {
    if (modelHasTerm(model, "preview")) return 9;
    if (model.endsWith("-high")) return 1;
    if (model.endsWith("-medium")) return 2;
    if (model.endsWith("-low")) return 3;
  }
  if (family === "grok") {
    if (model.endsWith("-non-reasoning")) return 1;
    if (model.endsWith("-reasoning")) return 0;
  }
  return 0;
}

function versionTokenComponents(token: string) {
  return (token.match(/\d+/g) ?? [])
    .filter((part) => part.length <= 5)
    .slice(0, 4)
    .map(Number);
}

function modelVersionComponents(family: SemanticModelFamily, model: string) {
  const tokens = model.split("-");
  const firstVersionToken = tokens.findIndex((token) => /\d/.test(token));
  if (firstVersionToken < 0) return [];
  if (family !== "anthropic") return versionTokenComponents(tokens[firstVersionToken] ?? "");

  const version: number[] = [];
  for (const token of tokens.slice(firstVersionToken)) {
    if (/^\d{6,}$/.test(token)) break;
    const components = versionTokenComponents(token);
    if (!components.length) break;
    version.push(...components);
    if (version.length >= 4) break;
  }
  return version.slice(0, 4);
}

function modelVersionRank(family: SemanticModelFamily, model: string) {
  const version = modelVersionComponents(family, model);
  const [, versionToken, release] = model.split("-");
  if (family === "grok" && /^\d+(?:\.\d+)*$/.test(versionToken ?? "") && /^\d{4}$/.test(release ?? "")) {
    const minor = version[1];
    if (minor != null && minor >= 10 && minor % 10 === 0) version[1] = minor / 10;
  }
  return [...version.map((part) => -part), ...Array(Math.max(0, 4 - version.length)).fill(0)];
}

function semanticModelSortKey(model: string): SemanticModelSortKey | null {
  const id = modelLeaf(model);
  const family = semanticModelFamily(id);
  if (!family) return null;
  const isImage = modelHasTerm(id, "image") || id.startsWith("dall-e");
  return {
    familyRank: modelFamilyRank(family),
    imageRank: Number(isImage),
    tierRank: modelTierRank(family, id, isImage),
    versionRank: modelVersionRank(family, id),
    modifierRank: modelModifierRank(family, id),
    previewRank: Number(modelHasTerm(id, "preview")),
    id,
  };
}

function compareNumberArrays(left: readonly number[], right: readonly number[]) {
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const delta = (left[index] ?? 0) - (right[index] ?? 0);
    if (delta) return delta;
  }
  return 0;
}

function compareSemanticModelKeys(left: SemanticModelSortKey, right: SemanticModelSortKey) {
  return left.familyRank - right.familyRank
    || left.imageRank - right.imageRank
    || left.tierRank - right.tierRank
    || compareNumberArrays(left.versionRank, right.versionRank)
    || left.modifierRank - right.modifierRank
    || left.previewRank - right.previewRank
    || left.id.localeCompare(right.id);
}

function launcherGroupRank(model: string, nativeChatGpt: boolean) {
  const group = modelProviderGroup(model, nativeChatGpt);
  if (group === "chatgpt") return 0;
  if (group === "openai") return 1;
  if (group === "anthropic") return 2;
  const family = semanticModelFamily(modelLeaf(model));
  if (family === "gemini") return 3;
  if (family === "grok") return 4;
  if (family === "zai") return 5;
  return group === otherGroup ? Number.MAX_SAFE_INTEGER : 6;
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
  const groupOrder = launcherGroupRank(left, leftNativeChatGpt) - launcherGroupRank(right, rightNativeChatGpt);
  if (groupOrder) return groupOrder;
  const leftKey = semanticModelSortKey(left);
  const rightKey = semanticModelSortKey(right);
  if (leftKey && rightKey) return compareSemanticModelKeys(leftKey, rightKey);
  if (leftKey) return -1;
  if (rightKey) return 1;
  return 0;
}

/// Launcher-only presentation ordering. Familiar model families use the same
/// semantic hierarchy as the public catalog; unknown IDs retain source order.
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
