import { describe, expect, test } from "bun:test";
import { groupModels, modelProviderGroup, supportsCacheWritePricing } from "../src/features/relay/modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../src/features/relay/modelPricing";

describe("model pricing", () => {
  test("groups provider families without hardcoding model versions", () => {
    expect(modelProviderGroup("gpt-5.7-future")).toBe("openai");
    expect(modelProviderGroup("claude-opus-4-8")).toBe("anthropic");
    expect(supportsCacheWritePricing("claude-sonnet-next")).toBe(true);
    expect(groupModels(["glm-5", "gpt-5", "gemini-3", "new-provider"], (model) => model).map((group) => group.id)).toEqual(["openai", "google", "zhipu", "other"]);
  });

  test("converts editable USD prices to integer micro-USD", () => {
    expect(parseEditableModelPrice("1.4")).toBe(1_400_000);
    expect(parseEditableModelPrice("1,4")).toBe(1_400_000);
    expect(formatEditableModelPrice(4_200_000)).toBe("4.2");
    expect(parseEditableModelPrice("1.0000001")).toBeNull();
    expect(parseOptionalEditableModelPrice("")).toBeNull();
  });
});
