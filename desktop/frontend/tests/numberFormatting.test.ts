import { describe, expect, test } from "bun:test";
import { formatNumber, getNumberFormatter } from "../src/features/relay/numberFormatting";

describe("number formatting cache", () => {
  test("preserves locale and precision", () => {
    expect(formatNumber(1234.567, "en-US", { maximumFractionDigits: 1 }))
      .toBe(new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 }).format(1234.567));
    expect(formatNumber(0.125, "de-DE", { style: "percent", maximumFractionDigits: 1 }))
      .toBe(new Intl.NumberFormat("de-DE", { style: "percent", maximumFractionDigits: 1 }).format(0.125));
  });

  test("reuses equivalent options regardless of insertion order", () => {
    const first = getNumberFormatter("en-US", { maximumFractionDigits: 2, minimumFractionDigits: 1 });
    const second = getNumberFormatter("en-US", { minimumFractionDigits: 1, maximumFractionDigits: 2 });
    expect(second).toBe(first);
  });
});
