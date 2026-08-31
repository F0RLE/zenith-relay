import { describe, expect, test } from "bun:test";
import {
  codexRequestOriginFromErrorCategory,
  normalizeObservedServiceTier,
  totalsFromRows,
  usageRowsFromLocal,
  usageRowsFromRemote,
  type UsageRow,
} from "../src/features/relay/pages/usage/usageData";
import type { LocalUsage, RemoteUsage } from "../src/features/relay/api/types";
import { formatUsageApiEquivalent } from "../src/features/relay/pages/usage/usageFormatting";

const row = (overrides: Partial<UsageRow> = {}): UsageRow => ({
  id: "request",
  attempt: 1,
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

  test("keeps safe provider tier diagnostics without changing their meaning", () => {
    expect(normalizeObservedServiceTier("default")).toBe("default");
    expect(normalizeObservedServiceTier("standard")).toBe("standard");
    expect(normalizeObservedServiceTier("priority")).toBe("priority");
    expect(normalizeObservedServiceTier("fast")).toBe("fast");
    expect(normalizeObservedServiceTier("flex")).toBe("flex");
    expect(normalizeObservedServiceTier("ultrafast")).toBe("ultrafast");
    expect(normalizeObservedServiceTier("not a tier")).toBeNull();
    expect(normalizeObservedServiceTier(null)).toBeNull();
  });

  test("uses one field projection for local and remote usage rows", () => {
    const local: LocalUsage = {
      id: 1,
      createdAt: "2026-08-23T00:00:00.000Z",
      requestId: "local-request",
      attempt: 2,
      sourceId: "source-1",
      accountId: "account-1",
      requestedModel: "gpt-5.4",
      resolvedModel: "gpt-5.4",
      wireApi: "responses",
      serviceTier: "fast",
      appliedServiceTier: "priority",
      success: true,
      httpStatus: 200,
      errorCategory: "codex_activity_summary",
      latencyMs: 900,
      ttftMs: 120,
      generationMs: 600,
      inputTokens: 100,
      cachedInputTokens: 20,
      reasoningTokens: 30,
      outputTokens: 40,
      totalTokens: 140,
    };
    const remote: RemoteUsage = {
      id: 2,
      requestId: "remote-request",
      attempt: 3,
      candidateKind: "account",
      candidateHint: "remote-account",
      candidateLabel: "Remote account",
      requestedModel: "gpt-5.4",
      resolvedModel: "gpt-5.4",
      wireApi: "responses",
      serviceTier: "standard",
      appliedServiceTier: "flex",
      success: true,
      httpStatus: 200,
      errorCategory: null,
      latencyMs: 800,
      ttftMs: 100,
      generationMs: 500,
      inputTokens: 90,
      cachedInputTokens: 10,
      reasoningTokens: 20,
      outputTokens: 30,
      totalTokens: 120,
      createdAtMs: Date.parse("2026-08-23T01:00:00.000Z"),
    };
    const labels = {
      backgroundConnection: "ChatGPT",
      unknownAccount: "Unknown account",
      removedAccount: "Removed account",
      unknownConnection: "Unknown connection",
    };

    expect(usageRowsFromLocal([local], {
      ...labels,
      accountLabels: new Map([["account-1", "Primary account"]]),
      sourceLabels: new Map([["source-1", "Primary source"]]),
    })[0]).toMatchObject({
      id: 1,
      attempt: 2,
      connection: "ChatGPT",
      accountId: "account-1",
      candidateKind: "account",
      appliedServiceTier: "priority",
      requestOrigin: "activity_summary",
      ttft: 120,
      generationMs: 600,
    });
    expect(usageRowsFromRemote([remote], {
      ...labels,
      accountDisplayName: (label) => label ? `Named ${label}` : null,
    })[0]).toMatchObject({
      id: 2,
      attempt: 3,
      connection: "Named Remote account",
      accountId: null,
      candidateKind: "account",
      appliedServiceTier: "flex",
      requestOrigin: null,
      ttft: 100,
      generationMs: 500,
    });
  });

  test("labels deleted account history without collapsing it into an unknown account", () => {
    const labels = {
      backgroundConnection: "ChatGPT",
      unknownAccount: "Unknown account",
      removedAccount: "Removed account",
      unknownConnection: "Unknown connection",
    };
    const local = {
      id: 3,
      createdAt: "2026-08-23T02:00:00.000Z",
      requestId: "deleted-local",
      attempt: 1,
      sourceId: "source-1",
      accountId: "deleted-account",
      requestedModel: "gpt-5.4",
      resolvedModel: "gpt-5.4",
      wireApi: "responses",
      serviceTier: "standard" as const,
      success: true,
      httpStatus: 200,
      errorCategory: null,
      latencyMs: 100,
      inputTokens: 1,
      outputTokens: 1,
      totalTokens: 2,
    } satisfies LocalUsage;
    const remote = {
      id: 4,
      requestId: "deleted-remote",
      attempt: 1,
      candidateKind: "account" as const,
      candidateHint: "deleted-account",
      candidateLabel: null,
      requestedModel: "gpt-5.4",
      resolvedModel: "gpt-5.4",
      wireApi: "responses",
      serviceTier: "standard" as const,
      success: true,
      httpStatus: 200,
      errorCategory: null,
      latencyMs: 100,
      inputTokens: 1,
      outputTokens: 1,
      totalTokens: 2,
      createdAtMs: Date.parse("2026-08-23T02:00:00.000Z"),
    } satisfies RemoteUsage;

    expect(usageRowsFromLocal([local], {
      ...labels,
      accountLabels: new Map(),
      sourceLabels: new Map([["source-1", "Primary source"]]),
    })[0]?.connection).toBe("Removed account");
    expect(usageRowsFromRemote([remote], {
      ...labels,
      accountDisplayName: () => null,
    })[0]?.connection).toBe("Removed account");
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
