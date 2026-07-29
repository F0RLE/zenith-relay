export type ModelProviderGroup = "openai" | "anthropic" | "google" | "xai" | "zhipu" | "other";

const groupOrder: ModelProviderGroup[] = ["openai", "anthropic", "google", "xai", "zhipu", "other"];

export function modelProviderGroup(model: string): ModelProviderGroup {
  const id = model.trim().toLowerCase();
  if (id.startsWith("claude-")) return "anthropic";
  if (id.startsWith("gemini-")) return "google";
  if (id.startsWith("grok-")) return "xai";
  if (id.startsWith("glm-")) return "zhipu";
  if (/^(gpt-|codex-|o\d|text-|dall-e)/.test(id)) return "openai";
  return "other";
}

export function groupModels<T>(items: T[], model: (item: T) => string) {
  const groups = new Map<ModelProviderGroup, T[]>();
  for (const item of items) {
    const group = modelProviderGroup(model(item));
    const values = groups.get(group);
    if (values) values.push(item);
    else groups.set(group, [item]);
  }
  return groupOrder.flatMap((id) => groups.has(id) ? [{ id, items: groups.get(id)! }] : []);
}

export function supportsCacheWritePricing(model: string) {
  return modelProviderGroup(model) === "anthropic";
}
