import { describe, expect, test } from "bun:test";
import { formatQuotaRemaining, formatWindowDuration } from "../src/features/relay/quotaFormatting";

describe("quota formatting", () => {
  test("formats reported windows using stable human units", () => {
    expect(formatWindowDuration(10_080, "en-US", "Window")).toBe("1 week");
    expect(formatWindowDuration(90, "en-US", "Window")).toBe("90 minutes");
    expect(formatWindowDuration(null, "en-US", "Window")).toBe("Window");
  });

  test("formats remaining quota as a percentage without inventing unknown values", () => {
    expect(formatQuotaRemaining(7_500, "en-US")).toBe("75%");
    expect(formatQuotaRemaining(null, "en-US")).toBe("—");
  });
});
