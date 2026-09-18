import { describe, expect, test } from "bun:test";
import {
  groupModels,
  orderModelIdsBySnapshot,
  uniqueModelIds,
} from "../src/features/relay/modelGroups";
import {
  formatEditableModelPrice,
  parseEditableModelPrice,
  parseOptionalEditableModelPrice,
} from "../src/features/relay/modelPricing";
import type { ModelSummary } from "../src/features/relay/api/types";

function metadata(
  id: string,
  catalogProvider: string | null,
  catalogFamily: string | null,
): Pick<ModelSummary, "id" | "catalogProvider" | "catalogFamily"> {
  return { id, catalogProvider, catalogFamily };
}

describe("model metadata presentation", () => {
  test("keeps all company families in one stable group in snapshot order", () => {
    const models = [
      metadata("new", "openai", "gpt-astra"),
      metadata("sol", "openai", "gpt-sol"),
      metadata("claude", "anthropic", "claude-opus"),
      metadata("terra", "openai", "gpt-terra"),
      metadata("mini", "openai", "gpt-mini"),
      metadata("unclassified", "openai", null),
    ];
    const groups = groupModels(models, { metadata: (model) => model });
    expect(groups.map((group) => [group.id, group.label, group.items.map((item) => item.id)]))
      .toEqual([
        ["catalog-openai", "OpenAI", ["new", "sol", "terra", "mini", "unclassified"]],
        ["catalog-anthropic", "Anthropic", ["claude"]],
      ]);
    const changedFamily = models.map((model) => ({ ...model, catalogFamily: "renamed" }));
    expect(groupModels(changedFamily, { metadata: (model) => model }).map((group) => group.id))
      .toEqual(groups.map((group) => group.id));
  });

  test("groups by backend metadata without parsing model IDs", () => {
    const models = [
      metadata("anything-1", "google", "gemini-flash"),
      metadata("private", null, null),
      metadata("anything-2", " GOOGLE ", "gemini-pro"),
      metadata("future", "new-lab", "new-family"),
    ];

    expect(groupModels(models, { metadata: (model) => model })
      .map((group) => [group.label, group.items.map((model) => model.id)]))
      .toEqual([
        ["Google", ["anything-1", "anything-2"]],
        ["Other", ["private"]],
        ["New Lab", ["future"]],
      ]);
  });

  test("formats catalog identity without changing its grouping key", () => {
    const models = [
      metadata("gpt", "openai", "gpt"),
      metadata("grok", "xai", "grok"),
      metadata("glm", "zai", "glm"),
    ];

    expect(groupModels(models, { metadata: (model) => model }).map((group) => group.label))
      .toEqual(["OpenAI", "xAI", "Z.ai"]);
  });

  test("merges native ChatGPT and catalog OpenAI models visually", () => {
    const models = [
      { ...metadata("native-new", null, null), nativeChatGpt: true },
      { ...metadata("api-model", "openai", "gpt-sol"), nativeChatGpt: false },
    ];

    expect(groupModels(models, {
      metadata: (model) => model,
      isNativeChatGpt: (model) => model.nativeChatGpt,
    }).map((group) => [group.provider, group.items.map((model) => model.id)]))
      .toEqual([["openai", ["native-new", "api-model"]]]);
  });

  test("keeps backend order and appends history-only models", () => {
    const summaries = [
      metadata("new", "openai", "gpt"),
      metadata("old", "openai", "gpt"),
    ] as ModelSummary[];
    expect(orderModelIdsBySnapshot(
      ["history", "old", "NEW", "history", "removed"],
      summaries,
    )).toEqual(["NEW", "old", "history", "removed"]);
    expect(uniqueModelIds([" First ", "first", "Second"])).toEqual([" First ", "Second"]);
  });
});

describe("model pricing", () => {
  test("converts editable USD prices to integer micro-USD", () => {
    expect(parseEditableModelPrice("1.4")).toBe(1_400_000);
    expect(parseEditableModelPrice("1,4")).toBe(1_400_000);
    expect(formatEditableModelPrice(4_200_000)).toBe("4.2");
    expect(parseEditableModelPrice("1.0000001")).toBeNull();
    expect(parseOptionalEditableModelPrice("")).toBeNull();
  });
});
