import { describe, expect, test } from "bun:test";
import { checkoutSettings } from "../../src/api/paymentBuilder";

describe("checkout draft amounts", () => {
  test("preserves ordered USD cents and decimal comma input", () => {
    expect(checkoutSettings("2,5", "20; 2.50 10,25", false, "2")).toEqual({
      minimum_credit_minor: 250, quick_amounts: [2000, 250, 1025], custom_amount: false, open_limit: 2,
    });
    expect(checkoutSettings("0.01", "", true, "10")?.quick_amounts).toEqual([]);
  });

  test("rejects duplicate, below-minimum, imprecise and unbounded selections", () => {
    for (const input of [
      ["1", "2 2.00", true, "3"], ["2", "1", true, "3"], ["0", "1", true, "3"],
      ["1", "2.001", true, "3"], ["1", "", false, "3"], ["1", "2", true, "11"],
      ["1", "2", true, "3e0"], ["10001", "", true, "1"], ["1", "1 2 3 4 5 6 7 8 9", true, "1"],
    ] as const) expect(checkoutSettings(...input)).toBeNull();
  });
});
