import { describe, expect, test } from "bun:test";
import { formatQuotaRemaining, formatWindowDuration, quotaWindowLabel } from "../src/features/relay/quotaFormatting";

const countTranslation = ((key: string, options?: { count?: number }) => `${key}:${options?.count ?? ""}`) as never;

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

  test("does not round reported quota windows up to a larger unit", () => {
    expect(quotaWindowLabel({ windowMinutes: 61 } as never, "primary", countTranslation)).toBe("quota.minutes:61");
    expect(quotaWindowLabel({ windowMinutes: 1_441 } as never, "primary", countTranslation)).toBe("quota.minutes:1441");
    expect(quotaWindowLabel({ windowMinutes: 120 } as never, "primary", countTranslation)).toBe("quota.hours:2");
  });
});
