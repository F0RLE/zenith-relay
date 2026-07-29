import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { en } from "../src/i18n/locales/en";
import { ru } from "../src/i18n/locales/ru";
import type { AccountSummary } from "../src/features/relay/api/types";
import { currentAccountErrorCode, formatDetailedRemainingTime, formatRemainingTime, quotaWindowLabel } from "../src/features/relay/components/Ui";

describe("Relay translations", () => {
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

  test("quota windows use ceiling-based units", () => {
    const t = ((key: string, options?: { count?: number }) => `${key}:${options?.count ?? ""}`) as never;
    expect(quotaWindowLabel({ windowMinutes: 43_800 } as never, "secondary", t)).toBe("quota.weeks:5");
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
