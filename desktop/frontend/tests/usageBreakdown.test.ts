import { describe, expect, test } from "bun:test";
import { usageBreakdown } from "../src/features/relay/pages/usage/usageBreakdown";

describe("usageBreakdown", () => {
  test("keeps reads inside input but does not price the remainder as regular when writes are unknown", () => {
    expect(usageBreakdown({
      inputTokens: 84_507,
      cachedInputTokens: 3_840,
      cacheWriteInputTokens: null,
      outputTokens: 791,
      reasoningTokens: 291,
      totalTokens: 85_298,
    })).toEqual({
      inputTotal: 84_507,
      uncachedInput: null,
      cacheRead: 3_840,
      cacheWrite: null,
      outputTotal: 791,
      reasoning: 291,
      visibleOutput: 500,
      total: 85_298,
    });
  });

  test("normalizes Anthropic cache components without double counting", () => {
    expect(usageBreakdown({
      inputTokens: 160,
      cachedInputTokens: 40,
      cacheWriteInputTokens: 20,
      outputTokens: 10,
      reasoningTokens: null,
      totalTokens: 170,
    })).toMatchObject({
      inputTotal: 160,
      uncachedInput: 100,
      cacheRead: 40,
      cacheWrite: 20,
      outputTotal: 10,
      total: 170,
    });
  });

  test("falls back to canonical total only when it was not reported", () => {
    expect(usageBreakdown({
      inputTokens: 12,
      cachedInputTokens: null,
      cacheWriteInputTokens: null,
      outputTokens: 8,
      reasoningTokens: 20,
      totalTokens: null,
    })).toMatchObject({ total: 20, reasoning: 8, visibleOutput: 0 });
  });

  test("cache components alone do not establish an input total or regular input", () => {
    expect(usageBreakdown({
      inputTokens: null,
      cachedInputTokens: 40,
      cacheWriteInputTokens: 20,
      outputTokens: 10,
      reasoningTokens: null,
      totalTokens: null,
    })).toMatchObject({ inputTotal: null, uncachedInput: null, cacheRead: 40, cacheWrite: 20, total: null });
  });

  test("a cache counter larger than the entire input is not rendered as a full cache hit", () => {
    expect(usageBreakdown({
      inputTokens: 8, cachedInputTokens: 10, cacheWriteInputTokens: null,
      outputTokens: 2, reasoningTokens: null, totalTokens: 10,
    })).toMatchObject({ inputTotal: 8, cacheRead: null, uncachedInput: null, total: 10 });
  });
});
