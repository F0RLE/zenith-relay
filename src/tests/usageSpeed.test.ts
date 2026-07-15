import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures generation after the first output token", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 });
    expect(speed).toBe(10);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("10 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 },
      { success: true, outputTokens: 20, durationMs: 500, generationDurationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBeCloseTo(28.75, 5);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("does not count hidden reasoning as visible output", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, reasoningTokens: 20, durationMs: 500, ttftMs: 100 })).toBeNull();
    expect(tokenSpeed({ success: true, outputTokens: 20, reasoningTokens: 5, durationMs: 500, ttftMs: 200 })).toBe(50);
  });
});
