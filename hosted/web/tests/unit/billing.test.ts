import { describe, expect, test } from "bun:test";
import { isTopUpAmount, parseTopUpAmount, quickTopUpAmounts } from "../../src/features/billing/model";

describe("demo top-up amounts", () => {
  test("whole dollars retain the existing 1–1000 range and exact microUSD", () => {
    expect(parseTopUpAmount("1")).toBe(1_000_000n);
    expect(parseTopUpAmount("1000")).toBe(1_000_000_000n);
    expect(parseTopUpAmount(" 25 ")).toBe(25_000_000n);
    expect(parseTopUpAmount("1e2")).toBe(100_000_000n);
    expect(parseTopUpAmount("25.0")).toBe(25_000_000n);
    for (const value of quickTopUpAmounts) expect(parseTopUpAmount(String(value))).toBe(BigInt(value) * 1_000_000n);
  });
  test("empty, fractional, unsafe and out-of-range values do not create a credit", () => {
    for (const value of ["", " ", "0", "-1", "1.5", "1001", "Infinity", "NaN", "money", "1e100", "9007199254740993"]) expect(parseTopUpAmount(value)).toBeNull();
  });
  test("the shell guard independently validates integer microUSD", () => {
    expect(isTopUpAmount(25_000_000n)).toBe(true);
    for (const value of [0n, -1n, 1n, 1_500_000n, 1_001_000_000n]) expect(isTopUpAmount(value)).toBe(false);
  });
});
