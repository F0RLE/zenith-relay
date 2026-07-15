import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures reported output over end-to-end latency", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 });
    expect(speed).toBeCloseTo(18.691588, 5);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("18.7 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 },
      { success: true, outputTokens: 20, durationMs: 500, generationDurationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBeCloseTo(30.172414, 5);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("keeps reasoning inside the official output token total", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, reasoningTokens: 20, durationMs: 500, ttftMs: 100 })).toBe(40);
    expect(tokenSpeed({ success: true, outputTokens: 20, reasoningTokens: 5, durationMs: 500, ttftMs: 200 })).toBe(40);
  });
});
