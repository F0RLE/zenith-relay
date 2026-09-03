import { describe, expect, test } from "bun:test";
import { averageTokenSpeed, formatTokenSpeed, tokenSpeed } from "../src/features/relay/usageSpeed";

describe("usage token speed", () => {
  test("measures tokens after the first output over generation time", () => {
    const speed = tokenSpeed({ success: true, outputTokens: 30, durationMs: 300 });
    expect(speed).toBeCloseTo(96.6667, 4);
    expect(formatTokenSpeed(speed, "en", "tok/s")).toBe("96.7 tok/s");
  });

  test("uses a token-weighted average and ignores failed samples", () => {
    expect(averageTokenSpeed([
      { success: true, outputTokens: 30, durationMs: 500 },
      { success: true, outputTokens: 20, durationMs: 500 },
      { success: false, outputTokens: 100, durationMs: 100 },
    ])).toBe(48);
    expect(averageTokenSpeed([{ success: false, outputTokens: 8, durationMs: 300 }])).toBeNull();
  });

  test("subtracts separately reported reasoning tokens", () => {
    expect(tokenSpeed({ success: true, outputTokens: 30, reasoningTokens: 10, durationMs: 300 })).toBeCloseTo(63.3333, 4);
  });

  test("uses the supplied generation duration and rejects one-token samples", () => {
    expect(tokenSpeed({ success: true, outputTokens: 20, durationMs: 800 })).toBeCloseTo(23.75, 4);
    expect(tokenSpeed({ success: true, outputTokens: 1, durationMs: 800 })).toBeNull();
    expect(tokenSpeed({ success: true, outputTokens: 20, durationMs: 0 })).toBeNull();
  });

  test("clamps malformed reasoning usage to reported output", () => {
    expect(tokenSpeed({ success: true, outputTokens: 4, reasoningTokens: 10, durationMs: 300 })).toBeNull();
  });

  test("ignores buffered outliers above the reasonable throughput limit", () => {
    expect(tokenSpeed({ success: true, outputTokens: 52, durationMs: 50 })).toBeNull();
    expect(tokenSpeed({ success: true, outputTokens: 51, durationMs: 51 })).toBeCloseTo(980.3922, 4);
    expect(averageTokenSpeed([
      { success: true, outputTokens: 52, durationMs: 50 },
      { success: true, outputTokens: 11, durationMs: 100 },
    ])).toBe(100);
  });
});
