import { expect, test } from "bun:test";
import { parsePurchaseLimit, purchaseLimitInput, purchaseLimitMicros } from "../../src/api/purchaseLimit";

test("purchase limit input preserves microUSD, explicit zero and the i64 boundary", () => {
  for (const [dollars, micros] of [["0", "0"], ["0.000001", "1"], ["12,0304", "12030400"],
    ["9223372036854.775807", "9223372036854775807"]] as const) {
    expect(purchaseLimitMicros(dollars)).toBe(micros);
    expect(purchaseLimitMicros(purchaseLimitInput(micros))).toBe(micros);
  }
  expect(purchaseLimitInput(null)).toBe("");
  for (const input of ["", "-1", "+1", "1e3", "1,000.00", "0.0000001", "9223372036854.775808"])
    expect(purchaseLimitMicros(input)).toBeUndefined();
});

test("owner budget keeps larger aggregate values exact and does not substitute unknown remaining", () => {
  const source = "1e0801cb-56cc-464f-89df-64befb1ed4e7";
  const row = { source_id: source, revision: "1", month_utc: "2026-11-01", limit_micro_usd: "0",
    accounted_micro_usd: "18446744073709551614", outstanding_micro_usd: "0", remaining_micro_usd: "0", unpriced: false };
  expect(parsePurchaseLimit(row, source)).toMatchObject({ accounted: 18_446_744_073_709_551_614n, remaining: 0n });
  expect(parsePurchaseLimit({ ...row, unpriced: true, remaining_micro_usd: null }, source).remaining).toBeNull();
  expect(parsePurchaseLimit({ ...row, limit_micro_usd: null, remaining_micro_usd: null }, source).remaining).toBeNull();
  for (const invalid of [{ source_id: "other" }, { month_utc: "2026-13-01" }, { outstanding_micro_usd: 0 },
    { unpriced: true }, { revision: "01" }, { limit_micro_usd: "9223372036854775808" }])
    expect(() => parsePurchaseLimit({ ...row, ...invalid }, source)).toThrow();
});
