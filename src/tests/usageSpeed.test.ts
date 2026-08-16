import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures output tokens after the first output", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 });
    expect(speed).toBe(10);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("10 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 8, reasoningTokens: 5, durationMs: 428, ttftMs: 128 },
      { success: true, outputTokens: 20, durationMs: 500, generationDurationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBe(28.75);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("prefers the measured generation duration", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, durationMs: 800, ttftMs: 300, generationDurationMs: 400 })).toBe(50);
  });

  test("does not use end-to-end time as streaming speed", () => {
    const sample = { success: true, outputTokens: 20, durationMs: 800 };
    expect(tokenSpeed(sample)).toBeNull();
    expect(averageTokenSpeed([sample])).toBeNull();
  });

  test("does not count hidden reasoning as streamed output", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, reasoningTokens: 5, durationMs: 600, ttftMs: 100 })).toBe(30);
  });
});
