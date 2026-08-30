import { describe, expect, test } from "bun:test";
import { relativeTimeRefreshDelay } from "../src/features/relay/hooks/useRelativeTimeClock";

describe("relative time clock", () => {
  test("stays idle without future timestamps", () => {
    expect(relativeTimeRefreshDelay([null, 10], 10)).toBeNull();
  });

  test("switches to second updates in the final hour", () => {
    const nowMs = 1_000_000;
    expect(relativeTimeRefreshDelay([nowMs + 60 * 60_000], nowMs)).toBe(60_000);
    expect(relativeTimeRefreshDelay([nowMs + 60 * 60_000 - 1], nowMs)).toBe(1_000);
  });
});
