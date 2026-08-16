import { describe, expect, test } from "bun:test";
import { groupModels, modelProviderGroup, modelProviderGroupLabel, sortModelIdsForLauncher, supportsCacheWritePricing } from "../src/features/relay/modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../src/features/relay/modelPricing";

describe("model pricing", () => {
  test("groups known and future provider families without a fixed model catalog", () => {
    expect(modelProviderGroup("gpt-5.7-future")).toBe("openai");
    expect(modelProviderGroup("claude-opus-4-8")).toBe("anthropic");
    expect(modelProviderGroup("grok-4.5")).toBe("provider-grok");
    expect(modelProviderGroup("gemini-3.6-flash")).toBe("provider-gemini");
    expect(modelProviderGroup("glm-5.2")).toBe("provider-glm");
    expect(modelProviderGroupLabel("provider-glm")).toBe("GLM");
    expect(supportsCacheWritePricing("claude-sonnet-next")).toBe(true);
    expect(groupModels(["glm-5", "gpt-5", "gemini-3", "grok-4", "new-provider"], (model) => model).map((group) => [group.id, group.items])).toEqual([
      ["openai", ["gpt-5"]],
      ["provider-gemini", ["gemini-3"]],
      ["provider-grok", ["grok-4"]],
      ["provider-glm", ["glm-5"]],
      ["provider-new", ["new-provider"]],
    ]);
  });

  test("uses semantic model order in source price editors and launchers", () => {
    expect(sortModelIdsForLauncher([
      "private-second",
      "grok-build-0.1",
      "grok-4.20-0309-non-reasoning",
      "grok-4.20-0309-reasoning",
      "grok-4.3",
      "grok-4.5",
      "grok-4.6",
      "gemini-2.5-flash-lite",
      "gemini-3.1-flash-lite",
      "gemini-2.5-flash",
      "gemini-3-flash-preview",
      "gemini-3-flash",
      "gemini-3.5-flash",
      "gemini-3.6-flash",
      "gemini-3.7-flash",
      "gemini-2.5-pro",
      "gemini-3-pro-preview",
      "gemini-3-pro",
      "gemini-3.1-pro-preview",
      "claude-haiku-4-5",
      "claude-sonnet-4-6",
      "claude-sonnet-5",
      "claude-opus-4-6",
      "claude-opus-4-7",
      "claude-opus-4-8",
      "claude-opus-5",
      "claude-fable-5",
      "gpt-5.4-mini",
      "gpt-5.4",
      "gpt-5.5",
      "gpt-5.6-terra",
      "gpt-5.6-sol",
      "private-first",
    ])).toEqual([
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.5",
      "gpt-5.4",
      "gpt-5.4-mini",
      "claude-fable-5",
      "claude-opus-5",
      "claude-opus-4-8",
      "claude-opus-4-7",
      "claude-opus-4-6",
      "claude-sonnet-5",
      "claude-sonnet-4-6",
      "claude-haiku-4-5",
      "gemini-3.1-pro-preview",
      "gemini-3-pro",
      "gemini-3-pro-preview",
      "gemini-2.5-pro",
      "gemini-3.7-flash",
      "gemini-3.6-flash",
      "gemini-3.5-flash",
      "gemini-3-flash",
      "gemini-3-flash-preview",
      "gemini-2.5-flash",
      "gemini-3.1-flash-lite",
      "gemini-2.5-flash-lite",
      "grok-4.6",
      "grok-4.5",
      "grok-4.3",
      "grok-4.20-0309-reasoning",
      "grok-4.20-0309-non-reasoning",
      "grok-build-0.1",
      "private-second",
      "private-first",
    ]);

    expect(sortModelIdsForLauncher([
      "grok-5.20-0612-non-reasoning",
      "grok-5.20-0612-reasoning",
      "grok-5.6",
      "grok-5.6.1",
      "gemini-4.1-flash",
      "gemini-4.1.2-flash",
      "gemini-3.9-pro",
      "gemini-4-pro",
      "claude-sonnet-6",
      "claude-opus-5",
      "claude-opus-6",
      "claude-opus-6-1",
      "gpt-6.2-terra",
      "gpt-6.2.1-terra",
      "gpt-6.2-sol",
      "gpt-6.2-experimental",
      "gpt-7.0",
    ])).toEqual([
      "gpt-7.0",
      "gpt-6.2.1-terra",
      "gpt-6.2-sol",
      "gpt-6.2-terra",
      "gpt-6.2-experimental",
      "claude-opus-6-1",
      "claude-opus-6",
      "claude-opus-5",
      "claude-sonnet-6",
      "gemini-4-pro",
      "gemini-3.9-pro",
      "gemini-4.1.2-flash",
      "gemini-4.1-flash",
      "grok-5.6.1",
      "grok-5.6",
      "grok-5.20-0612-reasoning",
      "grok-5.20-0612-non-reasoning",
    ]);
  });

  test("keeps native ChatGPT account models in the first dedicated group", () => {
    const models = [
      { id: "gpt-5.4-mini", nativeChatGpt: false },
      { id: "claude-opus-5", nativeChatGpt: false },
      { id: "gpt-5.6-sol", nativeChatGpt: true },
      { id: "gpt-5.4", nativeChatGpt: true },
    ];

    expect(
      groupModels(
        models,
        (model) => model.id,
        (model) => model.nativeChatGpt,
      ).map((group) => [group.id, group.items.map((model) => model.id)]),
    ).toEqual([
      ["chatgpt", ["gpt-5.6-sol", "gpt-5.4"]],
      ["openai", ["gpt-5.4-mini"]],
      ["anthropic", ["claude-opus-5"]],
    ]);
  });

  test("converts editable USD prices to integer micro-USD", () => {
    expect(parseEditableModelPrice("1.4")).toBe(1_400_000);
    expect(parseEditableModelPrice("1,4")).toBe(1_400_000);
    expect(formatEditableModelPrice(4_200_000)).toBe("4.2");
    expect(parseEditableModelPrice("1.0000001")).toBeNull();
    expect(parseOptionalEditableModelPrice("")).toBeNull();
  });
});
