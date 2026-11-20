import { describe, expect, test } from "bun:test";
import { parseModelStatus } from "../../src/api/modelStatus";

const step = 1_800_000;
const from = Date.parse("2024-03-01T12:34:56.789Z");
const cell = {
  state: "unknown", span_ms: step, monitored_ms: 0, operational_ms: 0,
  request_ms: 0, requests: null as number | null, cache_samples: 0, cache_bps: null as number | null,
  latency_ms: null, latency_samples: 0,
};

function response() {
  const last = { ...cell, request_ms: 80000, requests: 0 };
  const total = { ...last, span_ms: step * 48 };
  return {
    hours: 24, as_of_ms: from + step * 48,
    intervals: Array.from({ length: 48 }, (_, i) => ({ from_ms: from + i * step, to_ms: from + (i + 1) * step })),
    summary: total,
    items: [{
      model: "vendor/new:model", metrics: total,
      history: Array.from({ length: 48 }, (_, i) => i === 47 ? last : { ...cell }),
    }],
  };
}

describe("server model status", () => {
  test("server UTC bounds and unknown/zero are kept distinct", () => {
    const parsed = parseModelStatus(response(), 24);
    const history = parsed.items[0]!.intervals;
    expect(history[0]).toMatchObject({ startMs: from, metrics: { state: "unknown", requests: null } });
    expect(history[47]).toMatchObject({ endMs: from + step * 48, metrics: { state: "unknown", requests: 0 } });
    expect(parsed.summary).toMatchObject({ requests: 0, latencyMs: null, cacheBps: null });
  });

  test("rejects malformed windows, unobserved health and incomplete sample claims", () => {
    const bad = response();
    bad.intervals[4]!.from_ms++;
    expect(() => parseModelStatus(bad, 24)).toThrow();
    expect(() => parseModelStatus(response(), 168)).toThrow();
    const partial = response();
    Object.assign(partial.items[0]!.history[4]!, { state: "operational", monitored_ms: step / 2, operational_ms: step / 2 });
    expect(() => parseModelStatus(partial, 24)).toThrow();
    const inconsistent = response();
    Object.assign(inconsistent.items[0]!.history[4]!, { cache_samples: 1, cache_bps: 9000 });
    expect(() => parseModelStatus(inconsistent, 24)).toThrow();
  });
});
