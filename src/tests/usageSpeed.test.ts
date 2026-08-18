import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures all output tokens over the whole request", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 30, durationMs: 500 });
    expect(speed).toBe(60);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("60 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 30, durationMs: 500 },
      { success: true, outputTokens: 20, durationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBe(50);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("uses end-to-end time as the denominator", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, durationMs: 800 })).toBe(25);
  });
});
