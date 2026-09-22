import { describe, expect, test } from "bun:test";
import {
  apiProviderReady,
  apiProviderSourceInput,
  defaultApiProviderValue,
  selectApiProvider,
  type ApiProviderValue,
} from "../src/features/relay/components/apiProviderModel";

const provider = (overrides: Partial<ApiProviderValue> = {}): ApiProviderValue => ({
  ...defaultApiProviderValue(),
  kind: "custom",
  name: "Example",
  baseUrl: "https://example.test/v1",
  apiKey: "key",
  ...overrides,
});

describe("API provider model", () => {
  test("selects a provider without mutating the previous value or losing its key", () => {
    const current = provider({ apiKey: "preserve-me" });
    const selected = selectApiProvider(current, "openai");

    expect(selected).toMatchObject({ kind: "openai", name: "OpenAI", apiKey: "preserve-me" });
    expect(current.kind).toBe("custom");
  });

  test("validates provider identity and builds a trimmed source payload", () => {
    expect(apiProviderReady(provider({ apiKey: " " }))).toBe(false);
    expect(apiProviderReady(provider())).toBe(true);
    expect(apiProviderReady(provider({ name: "", baseUrl: "" }))).toBe(false);
    const preset = selectApiProvider(provider(), "zenith");
    expect(apiProviderReady(preset)).toBe(true);
    expect(apiProviderReady({ ...preset, name: " " })).toBe(false);
    expect(apiProviderReady({ ...preset, baseUrl: " " })).toBe(false);

    const input = apiProviderSourceInput(provider({ name: "  Example  ", baseUrl: " https://example.test/v1 ", apiKey: " key " }));
    expect(input).toMatchObject({ name: "Example", baseUrl: "https://example.test/v1", apiKey: "key", models: [] });
    expect(input.protocolBindings).toEqual([]);
  });

  test("uses the known provider protocol only as a discovery fallback", () => {
    const selected = selectApiProvider(provider(), "openrouter");
    expect(apiProviderSourceInput(selected)).toMatchObject({
      wireApi: "chat_completions",
      protocolBindings: [],
    });
  });
});
