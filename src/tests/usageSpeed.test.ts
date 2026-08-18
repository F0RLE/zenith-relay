import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures output tokens over generation time", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 30, durationMs: 300 });
    expect(speed).toBe(100);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("100 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 30, durationMs: 500 },
      { success: true, outputTokens: 20, durationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBe(50);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("uses the supplied generation duration as the denominator", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, durationMs: 800 })).toBe(25);
  });
});
