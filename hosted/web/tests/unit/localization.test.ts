import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { defaultLocale, formatMessage, isLocaleLoaded, isMessageKey, loadLocale, locales, resolveLocale, translate } from "../../src/i18n/catalog";
import english from "../../src/i18n/messages/en.json" with { type: "json" };
import type { MessageKey } from "../../src/i18n/types";
import { calendarDays, displayCalendarDate, parseCalendarInput } from "../../src/components/calendar-date";
import { formatMoney, formatDuration, formatSpeed, supportSummary, usageTotals } from "../../src/features/usage/model";
import { overviewTimeline } from "../../src/features/overview/timeline";
import { ownerAccountValue } from "../../src/features/owner/accountPresentation";
import { previewRequests } from "../../src/preview/fixtures";
import { previewOwnerAccount } from "../../src/preview/ownerAccounts";

const slots = (message: string) => [...message.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();

describe("English-first Hosted localization", () => {
  test("registered dictionaries share stable English keys and interpolation contracts", async () => {
    expect(defaultLocale).toBe("en");
    expect(locales.map(item => item.code)).toEqual(expect.arrayContaining(["en", "ru", "zh-CN"]));
    expect(new Set(locales.map(item => item.code)).size).toBe(locales.length);
    for (const entry of locales) {
      expect(resolveLocale(entry.code)).toBe(entry.code);
      const messages = await loadLocale(entry.code);
      expect(isLocaleLoaded(entry.code)).toBe(true);
      expect(await loadLocale(entry.code)).toBe(messages);
      expect(Object.keys(messages).sort()).toEqual(Object.keys(english).sort());
      for (const key of Object.keys(english) as MessageKey[]) {
        expect(key).toMatch(/^[a-z][a-zA-Z0-9]*(\.[a-zA-Z0-9]+)+$/);
        expect(messages[key]!.trim().length, `${entry.code}: ${key}`).toBeGreaterThan(0);
        expect(slots(messages[key]!), `${entry.code}: ${key}`).toEqual(slots(english[key]));
      }
    }
    const unregistered = ["fr", "ru-RU", "zh"].filter(code => !locales.some(entry => entry.code === code));
    for (const invalid of [null, undefined, "", "__unregistered_locale__", ...unregistered, {}, 1]) expect(resolveLocale(invalid)).toBe(defaultLocale);
    expect(isMessageKey("nav.overview")).toBe(true);
    expect(isMessageKey("Overview")).toBe(false);
    expect(isMessageKey("My custom project")).toBe(false);
    expect(translate("en", "connections.actionsFor", { name: "Проект 中文 $& {count}" })).toBe("Actions: Проект 中文 $& {count}");
  });

  test("missing or empty translations fall back to English without interpreting user text", () => {
    expect(formatMessage({}, "common.save")).toBe("Save");
    expect(formatMessage({ "common.save": "  " }, "common.save")).toBe("Save");
    expect(formatMessage({}, "connections.actionsFor", { name: "<script>$& {name}</script>" })).toBe("Actions: <script>$& {name}</script>");
    expect(formatMessage({}, "connections.actionsFor")).toBe("Actions: {name}");
  });

  test("valid translations may match English without losing interpolation", () => {
    expect(formatMessage({ "common.save": english["common.save"] }, "common.save")).toBe("Save");
    expect(formatMessage({ "connections.actionsFor": english["connections.actionsFor"] }, "connections.actionsFor", { name: "My project" })).toBe("Actions: My project");
  });

  test("non-English product copy stays in localization resources, not components or fixtures", () => {
    const root = join(import.meta.dir, "../../src");
    const violations: string[] = [];
    const scan = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        if (entry.name === "i18n") continue;
        const file = join(dir, entry.name);
        if (entry.isDirectory()) scan(file);
        else if (/\.(tsx?|css)$/.test(entry.name)) {
          readFileSync(file, "utf8").split("\n").forEach((line, index) => {
            if (/[\p{Script=Cyrillic}\p{Script=Han}]/u.test(line)) violations.push(`${file}:${index + 1}`);
            if (/ru-RU|zh-CN/.test(line) && /toLocale|Intl\./.test(line)) violations.push(`${file}:${index + 1}: pinned formatter`);
          });
        }
      }
    };
    scan(root);
    expect(violations).toEqual([]);
  });

  test("calendar input round-trips locale formats while retaining ISO and leap-day validity", () => {
    for (const [locale, entry, compact, start] of [
      ["en", "02/29/2028", "02292028", "2028-01-30"],
      ["ru", "29.02.2028", "29022028", "2028-01-31"],
      ["zh-CN", "2028/02/29", "20280229", "2028-01-31"],
    ] as const) {
      expect(parseCalendarInput(entry, locale)).toBe("2028-02-29");
      expect(parseCalendarInput(compact, locale)).toBe("2028-02-29");
      expect(parseCalendarInput("2028-02-29", locale)).toBe("2028-02-29");
      expect(parseCalendarInput(displayCalendarDate("2028-02-29", locale), locale)).toBe("2028-02-29");
      expect(calendarDays("2028-02-29", locale)[0]).toBe(start);
      for (const invalid of ["2027-02-29", "2028-04-31", "0000-01-01"]) expect(parseCalendarInput(invalid, locale)).toBeNull();
    }
    expect(parseCalendarInput("03/04/2028", "en")).toBe("2028-03-04");
    expect(parseCalendarInput("03.04.2028", "ru")).toBe("2028-04-03");
  });

  test("money remains exact, unknown is not zero, and formatting never changes statistics", async () => {
    const expected = usageTotals(previewRequests);
    const asOf = "2026-10-20T14:32:00Z";
    const boundaries = overviewTimeline(previewRequests, "week", asOf).buckets.map(({ startMs, endMs, rows }) => [startMs, endMs, rows.map(row => row.id)]);
    for (const { code } of locales) {
      await loadLocale(code);
      expect(formatMoney(null, 6, code)).toBe("—");
      expect(formatDuration(null, code)).toBe("—");
      expect(formatSpeed(null, code)).toBe("—");
      // Format the integer and fraction separately to keep the reference exact.
      const whole = new Intl.NumberFormat(code).format(9007199254n);
      const exact = new Intl.NumberFormat(code, { minimumFractionDigits: 6, maximumFractionDigits: 6 })
        .formatToParts(0.740993).map(part => part.type === "integer" ? whole : part.value).join("");
      expect(formatMoney(9007199254740993n, 6, code)).toBe(exact);
      expect(formatMoney(999999n, 2, code)).toBe(new Intl.NumberFormat(code, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(1));
      expect(overviewTimeline(previewRequests, "week", asOf, "all", code).buckets.map(({ startMs, endMs, rows }) => [startMs, endMs, rows.map(row => row.id)])).toEqual(boundaries);
      expect(supportSummary(previewRequests[0]!, code)).toContain(previewRequests[0]!.id);
      const account = previewOwnerAccount(Date.parse(asOf), "normal");
      expect(ownerAccountValue(account, Date.parse(asOf), code).paid).toBe(true);
    }
    expect(usageTotals(previewRequests)).toEqual(expected);
    expect(formatMoney(-1234567n, 6, "ru")).toBe("−1,234567");
    expect(formatMoney(0n, 6, "zh-CN")).toBe("0.000000");
  });
});
