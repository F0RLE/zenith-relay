import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../src/features/relay/api/types";
import { currentAccountErrorCode, formatDetailedRemainingTime, formatRemainingTime, quotaWindowLabel, transientCandidateTone } from "../src/features/relay/components/Ui";
import { sanitizeFeedbackError } from "../src/features/relay/state/feedback";

describe("Relay translations", () => {
  test("live routing status distinguishes cooldown and recovery probes", () => {
    const candidate = (overrides: Partial<CandidateRuntimeSnapshot> = {}) => ({
      candidateId: "source_1",
      kind: "api_source" as const,
      available: false,
      inFlight: 0,
      activeModels: [],
      lastUsedAtMs: null,
      nextRetryAtMs: null,
      halfOpen: false,
      dispatches: 0,
      ...overrides,
    });
    const nowMs = 1_000;

    expect(transientCandidateTone(candidate({ nextRetryAtMs: nowMs + 1 }), nowMs, true)).toBe("warning");
    expect(transientCandidateTone(candidate({ nextRetryAtMs: nowMs + 1 }), nowMs, false)).toBeNull();
    expect(transientCandidateTone(candidate({ halfOpen: true }), nowMs, true)).toBe("info");
    expect(transientCandidateTone(candidate({ nextRetryAtMs: nowMs - 1 }), nowMs, true)).toBeNull();
  });

  test("account cards prefer the latest quota refresh error in every view", () => {
    const account = {
      operationalStatus: "quotaWait",
      quotaRefreshStatus: "failed",
      routingBlockReason: "quota_exhausted",
      authState: { state: "degraded_access_only" },
      lastErrorCode: "upstream_quota_exhausted",
      quota: { error: { code: "credential_error", occurredAtMs: 123 } },
    } as unknown as AccountSummary;

    expect(currentAccountErrorCode(account)).toBe("credential_error");
  });

  test("feedback diagnostics redact token-shaped values before they reach the UI", () => {
    const result = sanitizeFeedbackError({
      code: "upstream_http_502",
      message: "Bearer live-secret api_key=sk-live-1234567890 token=abc123 Cookie: session=browser-secret; id_token=identity-secret x-api-key=provider-secret eyJhbGciOiJub25lIn0.eyJzdWIiOiIxIn0.signature",
    });

    expect(result.code).toBe("upstream_http_502");
    expect(result.message).not.toContain("live-secret");
    expect(result.message).not.toContain("sk-live-1234567890");
    expect(result.message).not.toContain("browser-secret");
    expect(result.message).not.toContain("identity-secret");
    expect(result.message).not.toContain("provider-secret");
    expect(result.message).not.toContain("eyJhbGciOiJub25lIn0");
    expect(result.message).toContain("Bearer [redacted]");
    expect(result.message).toContain("api_key=[redacted]");
  });

  test("feedback diagnostics keep only bounded routing context", () => {
    const result = sanitizeFeedbackError({
      code: "gateway_unavailable",
      message: "provider failed",
      diagnostic: {
        reason: "upstream",
        model: "claude-opus-5",
        source: "source_anthropic",
        route: "/v1/messages",
        requestId: "req_123",
        status: 429,
        retryable: true,
      },
      prompt: "must never be copied",
    });

    expect(result).toMatchObject({
      code: "gateway_unavailable",
      reason: "upstream",
      model: "claude-opus-5",
      source: "source_anthropic",
      route: "/v1/messages",
      requestId: "req_123",
      status: 429,
      retryable: true,
    });
    expect(JSON.stringify(result)).not.toContain("must never be copied");
  });

  test("English and Russian expose the same keys", () => {
    expect(flatten(en)).toEqual(flatten(ru));
  });

  test("every literal UI translation key exists", () => {
    const known = new Set(flatten(en));
    const used = new Set<string>();
    for (const file of walk(join(import.meta.dir, "..", "src"))) {
      const source = readFileSync(file, "utf8");
      for (const match of source.matchAll(/\bt\(\s*["']([^"']+)["']/g)) used.add(match[1]);
    }
    expect([...used].filter((key) => !known.has(key)).sort()).toEqual([]);
  });

  test("themed menus replace native select controls", () => {
    const nativeSelects = walk(join(import.meta.dir, "..", "src"))
      .filter((file) => readFileSync(file, "utf8").includes("<select"));
    expect(nativeSelects).toEqual([]);
  });

  test("remaining time keeps only the useful short units", () => {
    const t = ((key: string, options: { count: number }) => ru.timeShort[key.replace("timeShort.", "") as keyof typeof ru.timeShort].replace("{{count}}", String(options.count))) as never;
    const now = 1_000_000;
    expect(formatRemainingTime(now + 2 * 86_400_000, now, t)).toBe("2 дн.");
    expect(formatRemainingTime(now + 2 * 86_400_000 + 23 * 3_600_000, now, t)).toBe("2 дн.");
    expect(formatRemainingTime(now + 5 * 3_600_000 + 24 * 60_000 + 59_000, now, t)).toBe("5 ч 24 мин");
    expect(formatRemainingTime(now + 18 * 60_000 + 42_000, now, t)).toBe("18 мин 42 с");
    expect(formatRemainingTime(now + 9_000, now, t)).toBe("9 с");
  });

  test("subscription time includes days, hours, and minutes", () => {
    const t = ((key: string, options: { count: number }) => ru.timeShort[key.replace("timeShort.", "") as keyof typeof ru.timeShort].replace("{{count}}", String(options.count))) as never;
    const now = 1_000_000;
    expect(formatDetailedRemainingTime(now + 2 * 86_400_000, now, t)).toBe("2 дн. 0 ч 0 мин");
    expect(formatDetailedRemainingTime(now + 2 * 86_400_000 + 23 * 3_600_000 + 17 * 60_000, now, t)).toBe("2 дн. 23 ч 17 мин");
    expect(formatDetailedRemainingTime(now + 5 * 3_600_000 + 24 * 60_000, now, t)).toBe("5 ч 24 мин");
  });

  test("quota windows preserve their reported duration", () => {
    const t = ((key: string, options?: { count?: number }) => `${key}:${options?.count ?? ""}`) as never;
    expect(quotaWindowLabel({ windowMinutes: 43_800 } as never, "secondary", t)).toBe("quota.hours:730");
    expect(quotaWindowLabel({ windowMinutes: 10_080 } as never, "secondary", t)).toBe("quota.week:");
  });

});

function flatten(value: object, prefix = "", result: string[] = []) {
  for (const [key, item] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (item && typeof item === "object") flatten(item, path, result);
    else result.push(path);
  }
  return result.sort();
}

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = join(directory, name);
    return statSync(path).isDirectory() ? walk(path) : path.endsWith(".tsx") ? [path] : [];
  });
}
