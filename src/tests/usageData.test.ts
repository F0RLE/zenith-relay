import { describe, expect, test } from "bun:test";
import { createInstance } from "i18next";
import {
  codexRequestOriginFromErrorCategory,
  normalizeObservedServiceTier,
  logicalRequestRows,
  physicalRequestRows,
  totalsFromRows,
  usageRowsFromLocal,
  type UsageRow,
} from "../src/features/relay/pages/usage/usageData";
import {
  cacheLifetime,
  cacheTouchForRow,
  cacheWriteDurationWindows,
} from "../src/features/relay/pages/usage/cacheLifetime";
import type { LocalUsage } from "../src/features/relay/api/types";
import { formatUsageApiEquivalent } from "../src/features/relay/pages/usage/usageFormatting";
import { compactRequestId, formatModelId, formatReasoningEffort, formatReasoningSummary, formatTiming, prefixErrorOrigin, requestStatusKind, requestStatusLabel } from "../src/features/relay/pages/usage/usageReportFormat";
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
  test("catalog discovery is pending, not a failed or successful logical request", async () => {
    const pending = row({ success: false, routing: {
      reason: "quota_headroom", eligibleCandidates: 1, quotaRemainingBasisPoints: null,
      inFlightBefore: 0, dispatchesBefore: 0, attemptOutcome: "tool_discovery",
    } });
    expect(requestStatusKind(pending)).toBe("pending");
    expect(requestStatusKind({ ...pending, httpStatus: 502 })).toBe("error");
    expect(requestStatusKind({ ...pending, httpStatus: null })).toBe("error");
    expect(requestStatusKind({ ...pending, errorCategory: "adapter_upstream_response_invalid" })).toBe("error");
    expect(requestStatusKind({ ...pending, routing: null })).toBe("error");
    expect(requestStatusKind({ ...pending, success: true })).toBe("success");
    for (const [locale, translation, expected] of [
      ["ru", ru, "Поиск инструментов"],
      ["en", en, "Tool lookup"],
    ] as const) {
      const i18n = createInstance();
      await i18n.init({ lng: locale, resources: { [locale]: { translation } }, initImmediate: false });
      expect(requestStatusLabel(pending, i18n.t.bind(i18n))).toBe(expected);
    }
  });

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
        expect(local[0]).toMatchObject({ model: resolvedModel, requestedModel: alias, routedModel: resolvedModel });
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

  test("send rows preserve all usage and deduplicate only the same physical send", () => {
    const lookup = row({ id: 1, success: false, tokens: 15, inputTokens: 10, outputTokens: 5 });
    const final = row({ id: 2, attempt: 2, duration: 8000 });
    const corrected = { ...lookup, tokens: 25, inputTokens: 20, time: "2026-08-23T00:00:09Z" };
    const rows = physicalRequestRows([lookup, final, corrected, final]);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toBe(corrected);
    const totals = totalsFromRows(rows);
    expect(totals.requests).toBe(1);
    expect(totals.successfulRequests).toBe(1);
    expect(totals.totalTokens).toBe(70);
    expect(totals.latencyMs).toBe(8000);
    expect(totals.apiEquivalent.microUsd).toBe(3_000_000);
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

  test("uses only recorded windows, without inventing a provider policy", () => {
    expect(cacheWriteDurationWindows(null)).toEqual([]);
    expect(cacheWriteDurationWindows("30m")).toEqual(["30m"]);
    expect(cacheWriteDurationWindows("5m, 1h, 5m")).toEqual(["5m", "1h"]);
    expect(cacheWriteDurationWindows("15m")).toEqual(["15m"]);
    expect(cacheWriteDurationWindows("0m, unknown")).toEqual([]);
  });


  test("estimates cache lifetime from the last touch and the reported window", () => {
    const now = Date.parse("2026-09-27T12:00:00.000Z");
    expect(cacheLifetime({ cacheWriteTtl: "1h", touchedAt: "2026-09-27T11:20:00.000Z" }, now)).toMatchObject({
      windows: ["1h"], expiry: "open", remainingMs: 20 * 60_000,
    });
    expect(cacheLifetime({ cacheWriteTtl: "5m, 1h", touchedAt: "2026-09-27T11:10:00.000Z" }, now)).toMatchObject({
      windows: ["5m", "1h"], expiry: "unknown", remainingMs: null,
    });
    expect(cacheLifetime({ cacheWriteTtl: "5m", touchedAt: "2026-09-27T11:00:00.000Z" }, now).expiry).toBe("elapsed");
    expect(cacheLifetime({ cacheWriteTtl: null, touchedAt: "2026-09-27T11:50:00.000Z" }, now).expiry).toBe("unknown");
    expect(cacheLifetime({ cacheWriteTtl: "30m", touchedAt: "2026-09-27T11:40:00.000Z" }, now)).toMatchObject({
      windows: ["30m"], expiry: "open", remainingMs: 10 * 60_000,
    });
    expect(cacheLifetime({ cacheWriteTtl: "30m", touchedAt: "2026-09-27T11:00:00.000Z" }, now).expiry).toBe("elapsed");
    expect(cacheLifetime({ cacheWriteTtl: "15m", touchedAt: "2026-09-27T11:40:00.000Z" }, now).expiry).toBe("elapsed");
    expect(cacheLifetime({ cacheWriteTtl: "30m", touchedAt: "2026-09-27T12:10:00.000Z" }, now).remainingMs).toBe(30 * 60_000);
    expect(cacheLifetime({ cacheWriteTtl: "30m", touchedAt: "invalid" }, now).expiry).toBe("unknown");
  });

  test("cache reads refresh the observed window without becoming writes or borrowing another route", () => {
    const request = row({ model: "Display alias", routedModel: "upstream-model", clientContextId: "chat", time: "2026-10-18T12:25:00Z", cachedInputTokens: 100, cacheWriteInputTokens: 0 });
    const session = { clientContextId: "chat", candidateKind: "account" as const, candidateId: "account", model: "upstream-model", startedAt: "2026-10-18T12:00:00Z", touchedAt: "2026-10-18T12:00:00Z", cacheWriteTtl: "30m" };
    const touch = cacheTouchForRow(request, [session])!;
    expect(touch.touchedAt).toBe(request.time);
    expect(cacheLifetime(touch, Date.parse(request.time)).remainingMs).toBe(30 * 60_000);
    expect(cacheTouchForRow({ ...request, time: "2026-10-18T12:26:00Z", cachedInputTokens: 0 }, [{ ...session, touchedAt: request.time }])?.touchedAt).toBe(request.time);
    for (const foreign of [
      { ...session, clientContextId: "other" }, { ...session, model: "other" },
      { ...session, candidateId: "other" }, { ...session, candidateKind: "source" as const },
    ]) expect(cacheTouchForRow({ ...request, cachedInputTokens: 0 }, [foreign])).toBeNull();
    expect(cacheTouchForRow({ ...request, cachedInputTokens: 0 }, [])).toBeNull();
    const unknown = cacheTouchForRow({ ...request, model: "gpt-6", routedModel: "gpt-6" }, [])!;
    expect(cacheLifetime(unknown, Date.parse(request.time)).expiry).toBe("unknown");
    expect(request.cacheWriteInputTokens).toBe(0);
  });

  test("normalizes known Codex background categories", () => {
    expect(codexRequestOriginFromErrorCategory("codex_activity_summary")).toBe("activity_summary");
    expect(codexRequestOriginFromErrorCategory("codex_memory_extraction")).toBe("memory_extraction");
    expect(codexRequestOriginFromErrorCategory("codex_memory_consolidation")).toBe("memory_consolidation");
    expect(codexRequestOriginFromErrorCategory("codex_background_blocked_memory_extraction")).toBe("blocked_memory_extraction");
    expect(codexRequestOriginFromErrorCategory("codex_background_blocked_memory_consolidation")).toBe("blocked_memory_consolidation");
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

  test("projects local timing, tier and cache fields", () => {
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
      clientContextId: "client_0123456789abcdef01234567",
    });
    const upstreamError = { httpStatus: 422, code: "future_constraint", errorType: "validation_error", message: "Invalid field: temperature", redacted: false, truncated: false };
    for (const success of [false, true]) {
      const localRow = usageRowsFromLocal([{ ...local, success, upstreamError }], {
        ...labels, accountLabels: new Map(), sourceLabels: new Map(),
      })[0];
      expect(localRow?.upstreamError).toEqual(success ? null : upstreamError);
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
    expect(local[0]).toMatchObject({ model: "vendor/model", requestedModel: "public-alias", routedModel: "vendor/model" });
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

    expect(usageRowsFromLocal([local], {
      ...labels,
      accountLabels: new Map(),
      sourceLabels: new Map([["source-1", "Primary source"]]),
    })[0]?.connection).toBe("Removed account");
  });

  test("aggregates usage and marks rows without a price as unpriced", () => {
    const totals = totalsFromRows([
      row(),
      row({ id: "unpriced", requestId: "unpriced", success: false, tokens: 10, outputTokens: null, apiEquivalent: null }),
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
    expect(formatUsageApiEquivalent({ microUsd: 3_112_864_200, pricedTokens: 10, unpricedTokens: 2 }, "en-US", 2)).toBe("≈$3,112.86");
  });
});
