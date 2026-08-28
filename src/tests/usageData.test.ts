import { describe, expect, test } from "bun:test";
import {
  codexRequestOriginFromErrorCategory,
  totalsFromRows,
  type UsageRow,
} from "../src/features/relay/pages/usage/usageData";
import { formatUsageApiEquivalent } from "../src/features/relay/pages/usage/usageFormatting";

const row = (overrides: Partial<UsageRow> = {}): UsageRow => ({
  id: "request",
  time: "2026-08-23T00:00:00.000Z",
  success: true,
  model: "gpt-5.4",
  requestedReasoningEffort: null,
  effectiveReasoningEffort: null,
  connection: "Account",
  wireApi: "responses",
  serviceTier: "standard",
  appliedServiceTier: null,
  ttft: 100,
  generationMs: 500,
  duration: 700,
  inputTokens: 20,
  cachedInputTokens: null,
  cacheWriteInputTokens: null,
  cacheWriteTtl: null,
  reasoningTokens: 5,
  outputTokens: 25,
  tokens: 45,
  requestId: "request",
  httpStatus: 200,
  errorCategory: null,
  errorOrigin: null,
  toolUse: null,
  routing: null,
  accountId: "account",
  candidateKind: "account",
  apiEquivalent: { microUsd: 1_500_000, pricedTokens: 45, unpricedTokens: 0 },
  requestOrigin: null,
  ...overrides,
});

describe("usage data", () => {
  test("normalizes known Codex background categories", () => {
    expect(codexRequestOriginFromErrorCategory("codex_activity_summary")).toBe("activity_summary");
    expect(codexRequestOriginFromErrorCategory("other")).toBeNull();
  });

  test("aggregates usage and marks rows without a price as unpriced", () => {
    const totals = totalsFromRows([
      row(),
      row({ id: "unpriced", success: false, tokens: 10, outputTokens: null, apiEquivalent: null }),
    ]);

    expect(totals).toMatchObject({
      requests: 2,
      successfulRequests: 1,
      inputTokens: 40,
      outputTokens: 25,
      totalTokens: 55,
      apiEquivalent: { microUsd: 1_500_000, pricedTokens: 45, unpricedTokens: 10 },
    });
  });

  test("renders unavailable and partially priced API equivalents consistently", () => {
    expect(formatUsageApiEquivalent({ microUsd: 0, pricedTokens: 0, unpricedTokens: 10 }, "en-US")).toBe("-");
    expect(formatUsageApiEquivalent({ microUsd: 1_234_567, pricedTokens: 10, unpricedTokens: 2 }, "en-US")).toBe("≈$1.2346");
  });
});
