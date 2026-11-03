import { describe, expect, test } from "bun:test";
import { inUsagePeriod } from "../../src/features/usage/period";

describe("shared spending and history periods", () => {
  const asOf = "2026-10-30T12:00:00Z";
  test("24 hours is a rolling window, not the UTC calendar day", () => {
    expect(inUsagePeriod("2026-10-29T12:00:00Z", asOf, "24h")).toBe(true);
    expect(inUsagePeriod("2026-10-29T11:59:59.999Z", asOf, "24h")).toBe(false);
    expect(inUsagePeriod("2026-10-29T23:00:00Z", asOf, "today")).toBe(false);
    expect(inUsagePeriod("2026-10-30T00:00:00Z", asOf, "today")).toBe(true);
  });
  test("7 and 30 days keep exact elapsed windows across time-zone offsets", () => {
    expect(inUsagePeriod("2026-10-23T15:00:00+03:00", asOf, "week")).toBe(true);
    expect(inUsagePeriod("2026-10-23T11:59:59Z", asOf, "week")).toBe(false);
    expect(inUsagePeriod("2026-09-30T12:00:00Z", asOf, "month")).toBe(true);
    expect(inUsagePeriod("2026-09-30T11:59:59Z", asOf, "month")).toBe(false);
  });
  test("invalid and future dates do not enter totals; a payment at the snapshot does", () => {
    expect(inUsagePeriod(asOf, asOf, "24h")).toBe(true);
    expect(inUsagePeriod("2026-10-30T12:00:00.001Z", asOf, "all")).toBe(false);
    expect(inUsagePeriod("invalid", asOf, "all")).toBe(false);
    expect(inUsagePeriod(asOf, "invalid", "all")).toBe(false);
    expect(inUsagePeriod("2020-01-01T00:00:00Z", asOf, "all")).toBe(true);
  });
});
