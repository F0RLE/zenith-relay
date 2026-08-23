import { describe, expect, test } from "bun:test";
import { formatAccountValueMicroUsd, formatProviderMicroUsd } from "../src/features/relay/poolFormatting";

describe("pool currency formatting", () => {
  test("formats account value with optional approximation marker", () => {
    expect(formatAccountValueMicroUsd(1_234_567, "en-US")).toBe("$1.23");
    expect(formatAccountValueMicroUsd(1_234_567, "en-US", true)).toBe("≈$1.23");
    expect(formatAccountValueMicroUsd(12_000, "en-US")).toBe("$0.01");
  });

  test("keeps provider balance formatting at two decimal places", () => {
    expect(formatProviderMicroUsd(1_234_567, "en-US")).toBe("$1.23");
    expect(formatProviderMicroUsd(12_000, "en-US")).toBe("$0.01");
    expect(formatProviderMicroUsd(2_000_000, "en-US")).toBe("$2.00");
  });
});
