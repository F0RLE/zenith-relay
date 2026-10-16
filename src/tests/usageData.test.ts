import { describe, expect, test } from "bun:test";
import { createInstance } from "i18next";
import {
  codexRequestOriginFromErrorCategory,
  normalizeObservedServiceTier,
  logicalRequestRows,
  totalsFromRows,
  usageRowsFromLocal,
  usageRowsFromRemote,
  type UsageRow,
} from "../src/features/relay/pages/usage/usageData";
import {
  cacheLifetime,
  cacheWriteDurationWindows,
  documentedCacheRetentionMinimum,
} from "../src/features/relay/pages/usage/cacheLifetime";
import type { LocalUsage, RemoteUsage } from "../src/features/relay/api/types";
import { formatUsageApiEquivalent } from "../src/features/relay/pages/usage/usageFormatting";
import { compactRequestId, formatModelId, formatReasoningEffort, formatReasoningSummary, formatTiming, prefixErrorOrigin } from "../src/features/relay/pages/usage/usageReportFormat";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";

const row = (overrides: Partial<UsageRow> = {}): UsageRow => ({
  id: "request",
  attempt: 1,
  time: "2026-08-23T00:00:00.000Z",
  success: true,
  model: "gpt-5.4",
  requestedModel: "gpt-5.4",
  routedModel: "gpt-5.4",
  requestedReasoningEffort: null,
  effectiveReasoningEffort: null,
  connection: "Account",
  wireApi: "responses",
  transport: "http",
  serviceTier: "standard",
  appliedServiceTier: null,
  ttft: 100,
  generationMs: 500,
  duration: 700,
  inputTokens: 20,
  cachedInputTokens: null,
  cacheWriteInputTokens: null,
  cacheWriteTtl: null,
  documentedCacheRetentionMinimum: null,
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
  candidateKey: "account",
  apiEquivalent: { microUsd: 1_500_000, pricedTokens: 45, unpricedTokens: 0 },
  requestOrigin: null,
  clientContextId: null,
  ...overrides,
});

describe("usage data", () => {
  test("timing keeps unknown first output distinct from total duration and known zero", async () => {
    for (const [locale, translation, unknown, zero, known] of [
      ["ru", ru, "— / 20,9 с", "0 мс / 428 мс", "128 мс / 428 мс"],
      ["en", en, "— / 20.9 s", "0 ms / 428 ms", "128 ms / 428 ms"],
    ] as const) {
      const i18n = createInstance();
      await i18n.init({ lng: locale, resources: { [locale]: { translation } }, initImmediate: false });
      const t = i18n.t.bind(i18n);
      expect(formatTiming(null, 20_900, locale, t)).toBe(unknown);
      expect(formatTiming(0, 428, locale, t)).toBe(zero);
      expect(formatTiming(128, 428, locale, t)).toBe(known);
    }
  });

  test("decodes Relay model aliases for display without changing unknown IDs", () => {
    expect(formatModelId("zenith/Z2xtLTUuMw")).toBe("glm-5.3");
    expect(formatModelId("zenith/azMtMjU2aw")).toBe("k3-256k");
    expect(formatModelId("zenith/not-base64!")).toBe("zenith/not-base64!");
    expect(formatModelId("vendor/model")).toBe("vendor/model");
    expect(formatModelId("claude-fable-5")).toBe("claude-fable-5");
    expect(formatModelId("claude-sonnet-5")).toBe("claude-sonnet-5");
    expect(formatModelId("public-alias", "vendor/model")).toBe("public-alias");
  });

  test("Desktop aliases need a resolved model and retain raw IDs for both hosts", () => {
    const legacySuffix = Array(32).fill("ab").join("-");
    const aliases = [
      "claude-sonnet-5-relay-a1b2-c3d4-e5f6-7890",
      "claude-fable-5-relay-a1b2-c3d4-e5f6-7890",
      `claude-zenith-${legacySuffix}`,
      `claude-fable-5-zenith-${legacySuffix}`,
    ];
    const labels = {
      backgroundConnection: "ChatGPT", unknownAccount: "Unknown", removedAccount: "Removed",
      unknownConnection: "Unknown", accountLabels: new Map<string, string>(), sourceLabels: new Map<string, string>(),
      accountDisplayName: (label: string) => label,
    };
    for (const alias of aliases) {
      expect(formatModelId(alias)).toBe("—");
      expect(formatModelId(alias.toUpperCase(), "vendor/model")).toBe("vendor/model");
      expect(formatModelId(alias, "zenith/Z2xtLTUuMw")).toBe("glm-5.3");
      expect(formatModelId(alias, aliases[0])).toBe("—");
      for (const resolvedModel of [null, "vendor/model"]) {
        const base = {
          id: 1, requestId: "synthetic-request", attempt: 1,
          requestedModel: alias, resolvedModel,
          wireApi: "messages" as const, transport: "http" as const,
          success: resolvedModel !== null, httpStatus: resolvedModel ? 200 : 404, errorCategory: null,
          latencyMs: 1, inputTokens: null, cachedInputTokens: null,
          reasoningTokens: null, outputTokens: null, totalTokens: null,
        };
        const local = usageRowsFromLocal([{ ...base, createdAt: "2026-10-16T00:00:00Z", sourceId: "source" }], labels);
        const remote = usageRowsFromRemote([{ ...base, createdAtMs: 1, candidateKind: "source", candidateHint: "source" }], labels);
        for (const rows of [local, remote]) {
          expect(rows[0]).toMatchObject({ model: resolvedModel, requestedModel: alias, routedModel: resolvedModel });
        }
      }
    }
  });

  test("reasoning reports normalize historical aliases and show actual changes", async () => {
    const i18n = createInstance();
    await i18n.init({ lng: "ru", resources: { ru: { translation: ru } }, initImmediate: false });
    const t = i18n.t.bind(i18n);
    for (const effort of ["xhigh", "very_high", "extra-high", "Very high"]) {
      expect(formatReasoningEffort(effort, t)).toBe("Очень высокое");
    }
    expect(formatReasoningSummary({
      requestedReasoningEffort: "xhigh",
      effectiveReasoningEffort: "xhigh",
    }, t)).toBe("Очень высокое");
    expect(formatReasoningSummary(JSON.parse('{"requestedReasoningEffort":"very_high","effectiveReasoningEffort":"xhigh"}'), t)).toBe("Очень высокое");
    expect(formatReasoningSummary({
      requestedReasoningEffort: "high",
      effectiveReasoningEffort: "xhigh",
    }, t)).toBe("Запрошено: Высокое, передано: Очень высокое");
    expect(formatReasoningEffort("none", t)).toBe("Выключено");
  });

  test("reports keep one final result across connections and preserve separate client requests", () => {
    const final = row({ id: 3, attempt: 3, connection: "Fallback", time: "2026-08-23T00:00:03Z" });
    const other = row({ id: 4, requestId: "request-other", success: false, time: "2026-08-23T00:00:04Z" });
    const requests = logicalRequestRows([
      row({ id: 1, success: false, time: "2026-08-23T00:00:01Z" }),
      final,
      row({ id: 2, attempt: 2, success: false, time: "2026-08-23T00:00:09Z" }),
      other,
    ]);
    expect(requests).toEqual([other, final]);
    expect(totalsFromRows(requests).requests).toBe(2);
    expect(totalsFromRows(requests).successfulRequests).toBe(1);
    expect(logicalRequestRows([row({ id: 10, requestId: null }), row({ id: 11, requestId: null })])).toHaveLength(2);
  });

  test("short request labels preserve the differing suffix instead of repeating an identical prefix", () => {
    const first = "relay-synthetic-common-prefix-11111111-1";
    const second = "relay-synthetic-common-prefix-22222222-1";
    expect(compactRequestId(first)).not.toBe(compactRequestId(second));
    expect(compactRequestId(first)).toEndWith("11111111-1");
    expect(compactRequestId("short-id")).toBe("short-id");
  });

  test("usage error messages keep an English source prefix without duplication", () => {
    expect(prefixErrorOrigin("account", "Provider: upstream rejected request")).toBe(
      "Account: upstream rejected request",
    );
    expect(prefixErrorOrigin("relay", "Relay: request failed")).toBe("Relay: request failed");
    expect(prefixErrorOrigin(null, "historical request failed")).toBe("historical request failed");
  });

  test("keeps documented model minimum separate from provider-reported cache windows", () => {
    for (const model of ["gpt-5.6", "openai/gpt-5.6-2026-09-01", "gpt-6", "gpt-6-astra", "gpt-6-sol", "gpt-7-future"]) {
      expect(documentedCacheRetentionMinimum(model, 0, 10)).toBe("30m");
    }
    expect(documentedCacheRetentionMinimum("gpt-6-luna", 12, 0)).toBe("30m");
    expect(documentedCacheRetentionMinimum("gpt-6-astra", 12, 10, "1h")).toBeNull();
    expect(documentedCacheRetentionMinimum("gpt-6-astra", 12, 10, "5m")).toBeNull();
    expect(documentedCacheRetentionMinimum("gpt-6-astra", 0, 0)).toBeNull();
    expect(documentedCacheRetentionMinimum("gpt-5.5", 12, 10)).toBeNull();
    expect(documentedCacheRetentionMinimum("gpt-6x-preview", 12, 10)).toBeNull();
    expect(documentedCacheRetentionMinimum("gpt-6.bad-suffix", 12, 10)).toBeNull();
    expect(documentedCacheRetentionMinimum("claude-sonnet", 12, 10)).toBeNull();
  });

  test("keeps reported cache-write windows and uses 30 minutes only as the OpenAI fallback", () => {
    expect(cacheWriteDurationWindows(null, "30m")).toEqual(["30m"]);
    expect(cacheWriteDurationWindows("30m", null)).toEqual(["30m"]);
    expect(cacheWriteDurationWindows("5m, 1h", null)).toEqual(["5m", "1h"]);
    expect(cacheWriteDurationWindows("15m", "30m")).toEqual(["15m"]);
    expect(cacheWriteDurationWindows(null, null)).toEqual([]);
  });


  test("estimates cache lifetime from the last touch and the reported window", () => {
    const now = Date.parse("2026-09-27T12:00:00.000Z");
    expect(cacheLifetime({ model: "claude-sonnet-4", cacheWriteTtl: "1h", touchedAt: "2026-09-27T11:20:00.000Z" }, now)).toMatchObject({
      windows: ["1h"], expiry: "open", remainingMs: 20 * 60_000,
    });
    expect(cacheLifetime({ model: "claude-sonnet-4", cacheWriteTtl: "5m, 1h", touchedAt: "2026-09-27T11:10:00.000Z" }, now)).toMatchObject({
      windows: ["5m", "1h"], expiry: "open", remainingMs: 10 * 60_000,
    });
    expect(cacheLifetime({ model: "claude-opus", cacheWriteTtl: "5m", touchedAt: "2026-09-27T11:00:00.000Z" }, now).expiry).toBe("elapsed");
    expect(cacheLifetime({ model: "claude-opus", cacheWriteTtl: null, touchedAt: "2026-09-27T11:50:00.000Z" }, now).expiry).toBe("unknown");
    expect(cacheLifetime({ model: "gpt-6-astra", cacheWriteTtl: null, touchedAt: "2026-09-27T11:40:00.000Z" }, now)).toMatchObject({
      windows: ["30m"], expiry: "open", remainingMs: 10 * 60_000,
    });
    expect(cacheLifetime({ model: "gpt-6-astra", cacheWriteTtl: null, touchedAt: "2026-09-27T11:00:00.000Z" }, now).expiry).toBe("minimum_elapsed");
    expect(cacheLifetime({ model: "gpt-6", cacheWriteTtl: "30m", touchedAt: "2026-09-27T11:00:00.000Z" }, now).expiry).toBe("minimum_elapsed");
    expect(cacheLifetime({ model: "gpt-6", cacheWriteTtl: "15m", touchedAt: "2026-09-27T11:40:00.000Z" }, now).expiry).toBe("elapsed");
  });

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
      transport: "http",
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
      cacheWriteInputTokens: null,
      cacheWriteTtl: "45m",
      clientContextId: "client_0123456789abcdef01234567",
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
      transport: "websocket",
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
      cacheWriteInputTokens: null,
      cacheWriteTtl: "45m",
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
      candidateKey: "account-1",
      appliedServiceTier: "priority",
      requestOrigin: "activity_summary",
      ttft: 120,
      generationMs: 600,
      cacheWriteTtl: "45m",
      documentedCacheRetentionMinimum: null,
      clientContextId: "client_0123456789abcdef01234567",
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
      candidateKey: "remote-account",
      appliedServiceTier: "flex",
      cacheWriteTtl: "45m",
      documentedCacheRetentionMinimum: null,
      clientContextId: null,
      requestOrigin: null,
      ttft: 100,
      generationMs: 500,
    });
    const upstreamError = { httpStatus: 422, code: "future_constraint", errorType: "validation_error", message: "Invalid field: temperature", redacted: false, truncated: false };
    for (const success of [false, true]) {
      const localRow = usageRowsFromLocal([{ ...local, success, upstreamError }], {
        ...labels, accountLabels: new Map(), sourceLabels: new Map(),
      })[0];
      const remoteRow = usageRowsFromRemote([{ ...remote, success, upstreamError }], {
        ...labels, accountDisplayName: (label) => label,
      })[0];
      expect(localRow?.upstreamError).toEqual(success ? null : upstreamError);
      expect(remoteRow?.upstreamError).toEqual(success ? null : upstreamError);
    }
  });

  test("keeps the client model and routed source model separate for both hosts", () => {
    const base = {
      id: 1, requestId: "request", attempt: 1,
      requestedModel: "public-alias", resolvedModel: "vendor/model",
      wireApi: "responses" as const, transport: "http" as const, success: true, httpStatus: 200, errorCategory: null,
      latencyMs: 1, inputTokens: null, cachedInputTokens: null,
      reasoningTokens: null, outputTokens: null, totalTokens: null,
    };
    const local = usageRowsFromLocal([{ ...base, createdAt: "2026-08-23T00:00:00Z", sourceId: "source" }], {
      backgroundConnection: "ChatGPT", unknownAccount: "Unknown", removedAccount: "Removed",
      unknownConnection: "Unknown", accountLabels: new Map(), sourceLabels: new Map(),
    });
    const remote = usageRowsFromRemote([{ ...base, createdAtMs: 1, candidateKind: "source", candidateHint: "source" }], {
      backgroundConnection: "ChatGPT", unknownAccount: "Unknown", removedAccount: "Removed",
      unknownConnection: "Unknown", accountDisplayName: (label) => label,
    });
    for (const rows of [local, remote]) {
      expect(rows[0]).toMatchObject({ model: "vendor/model", requestedModel: "public-alias", routedModel: "vendor/model" });
    }
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
      transport: "http" as const,
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
      transport: "websocket" as const,
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
    expect(formatUsageApiEquivalent({ microUsd: 0, pricedTokens: 0, unpricedTokens: 10 }, "en-US")).toBe("—");
    expect(formatUsageApiEquivalent({ microUsd: 1_234_567, pricedTokens: 10, unpricedTokens: 2 }, "en-US")).toBe("≈$1.2346");
  });
});
