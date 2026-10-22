import { describe, expect, test } from "bun:test";
import { componentTokens, componentTotal, formatMoney, supportSummary, tokenTotal, usageTotals } from "../../src/features/usage/model";
import { previewRequests, priceDemoComponent } from "../../src/preview/fixtures";

describe("request tokens and money presentation", () => {
  test("cache and reasoning are not counted twice", () => {
    const tokens = { input: 2420, output: 435, cacheRead: 1064, cacheWrite: 84, reasoning: 65 };
    expect(tokenTotal(tokens)).toBe(2855);
    expect(componentTokens(tokens)).toEqual({ input: 1272, cacheRead: 1064, cacheWrite: 84, output: 435 });
    expect(tokens.output - tokens.reasoning).toBe(370);
  });

  test("unknown counters are not zero or an invented uncached count", () => {
    const tokens = { input: 2420, output: null, cacheRead: null, cacheWrite: 84, reasoning: null };
    expect(tokenTotal(tokens)).toBeNull();
    expect(componentTokens(tokens).input).toBeNull();
    expect(componentTotal({ input: 0n, output: null, cacheRead: 0n, cacheWrite: 0n })).toBeNull();
    expect(formatMoney(null)).toBe("—");
    expect(formatMoney(0n)).toBe("0,000000");
  });

  test("formatting preserves microUSD beyond Number.MAX_SAFE_INTEGER", () => {
    expect(formatMoney(9007199254740993n).replace(/\s/g, "")).toBe("9007199254,740993");
    expect(formatMoney(-1234567n)).toBe("−1,234567");
    expect(formatMoney(999999n, 2)).toBe("1,00");
    expect(formatMoney(60000n, 2)).toBe("0,06");
  });

  test("synthetic components use integer rounding, not floating point money", () => {
    expect(priceDemoComponent(1, 499999n)).toBe(0n);
    expect(priceDemoComponent(1, 500000n)).toBe(1n);
    expect(priceDemoComponent(1272, 400000n)).toBe(509n);
    expect(priceDemoComponent(null, 400000n)).toBeNull();
    for (const count of [-1, .5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => priceDemoComponent(count, 1n)).toThrow(RangeError);
    }
    expect(() => priceDemoComponent(1, -1n)).toThrow(RangeError);
  });

  test("all complete fixture breakdowns reconcile with their recorded charges", () => {
    for (const row of previewRequests) {
      expect(componentTotal(row.billing.components)).toBe(row.billing.postedMicroUsd);
      if (row.tokens.input !== null) {
        const parts = componentTokens(row.tokens);
        expect(parts.input! + parts.cacheRead! + parts.cacheWrite!).toBe(row.tokens.input);
        expect(row.tokens.reasoning!).toBeLessThanOrEqual(row.tokens.output!);
      }
    }
    expect(new Set(previewRequests.map(row => row.id)).size).toBe(previewRequests.length);
  });

  test("execution and accounting stay independent", () => {
    expect(previewRequests.some(row => row.execution === "completed" && row.accounting === "pending")).toBe(true);
    expect(previewRequests.filter(row => row.execution === "failed").every(row => row.billing.postedMicroUsd! > 0n)).toBe(true);
    expect(previewRequests.filter(row => row.accounting === "held").reduce((sum, row) => sum + row.billing.reservedMicroUsd!, 0n)).toBe(180000n);
  });

  test("summaries preserve missing posted charge and reconcile groups", () => {
    const original = previewRequests.find(row => row.accounting === "posted")!;
    expect(usageTotals([{ ...original, billing: { ...original.billing, postedMicroUsd: null } }]).charge).toBeNull();
    const total = usageTotals(previewRequests);
    const byKey = ["editor", "project", "bot"].map(id => usageTotals(previewRequests.filter(row => row.key.id === id)));
    expect(byKey.reduce((sum, group) => sum + group.charge!, 0n)).toBe(total.charge!);
    expect(byKey.reduce((sum, group) => sum + group.tokens, 0)).toBe(total.tokens);
    expect(usageTotals([]).charge).toBe(0n);
  });

  test("support copy uses an allowlist, not an object dump", () => {
    const original = previewRequests.find(row => row.execution === "failed")!;
    const contaminated = {
      ...original,
      client: { label: "PRIVATE_CLIENT", userAgent: "PRIVATE_AGENT" },
      rawPrompt: "SECRET_PROMPT", authorization: "SECRET_AUTH", accountEmail: "PRIVATE_ACCOUNT",
      response: { ...original.response, guidance: "PRIVATE_RAW_ERROR" },
    };
    const text = supportSummary(contaminated);
    expect(text).toContain(original.id);
    expect(text).toContain("response_interrupted");
    expect(text).not.toMatch(/PRIVATE_|SECRET_/);
  });
});
