import { expect, test } from "bun:test";
import { parseOwnerUsageOverview } from "../../src/api/ownerOverview";

const end = Date.UTC(2024, 2, 1, 0, 30, 0, 123);
const empty = { requests: "0", token_samples: "0", partial_tokens: "0", unsettled: "0", total_tokens: "0", spend_micro_usd: "0" };
const known = { ...empty, requests: "9007199254740993", token_samples: "1",
  total_tokens: "18446744073709551614", spend_micro_usd: "9007199254740993" };
const unknown = { ...empty, requests: "1", unsettled: "1", partial_tokens: "1", total_tokens: null };
const filter = { hours: 0, model: "fixture:v1/next" } as const;
const payload = () => ({
  hours: 0, model: filter.model, currency: "USD", precision: 6,
  from_ms: end - 301, as_of_ms: end, step_ms: 11, models: ["fixture", filter.model], more_models: false,
  totals: { ...known, requests: "9007199254740994", unsettled: "1", partial_tokens: "1" },
  buckets: [known, unknown, ...Array.from({ length: 26 }, () => empty)],
});

test("the exact owner values and clipped All time interval survive decoding", () => {
  const v = parseOwnerUsageOverview(payload(), filter);
  expect(v.totals.requests).toBe(9007199254740994n);
  expect(v.totals.spend).toBe(9007199254740993n);
  expect(v.totals.tokens).toBe(18446744073709551614n);
  expect(v.buckets.slice(0, 3).map(b => b.tokens)).toEqual([18446744073709551614n, null, 0n]);
  expect(v.buckets.at(-1)?.from).toBe(end - 4);
  expect(v.buckets.at(-1)?.to).toBe(end);
});

test("fixed windows end at the snapshot; invalid filters and mixed totals fail closed", () => {
  const p = payload();
  for (const [hours, count] of [[24, 24], [168, 7], [720, 30]] as const) {
    const step = hours === 24 ? 3600000 : 86400000;
    const fixed = { ...p, hours, from_ms: end - count * step, step_ms: step, model: null,
      buckets: Array.from({ length: count }, () => empty), totals: empty };
    const decoded = parseOwnerUsageOverview(fixed, { hours });
    expect(decoded.buckets).toHaveLength(count);
    expect(decoded.buckets[0]?.from).toBe(end - hours * 3600000);
    expect(decoded.buckets.at(-1)?.to).toBe(end);
    expect(() => parseOwnerUsageOverview({ ...fixed, as_of_ms: end + 1 }, { hours })).toThrow();
  }
  for (const invalid of [
    { ...p, model: "fixture" }, { ...p, hours: 24 }, { ...p, step_ms: 10 },
    { ...p, buckets: p.buckets.slice(1) }, { ...p, models: [filter.model, "fixture"] },
    { ...p, more_models: true }, { ...p, totals: { ...p.totals, requests: 9007199254740994 } },
    { ...p, totals: { ...p.totals, unsettled: "01" } }, { ...p, totals: { ...p.totals, spend_micro_usd: "9007199254740992" } },
    { ...p, buckets: [known, { ...unknown, total_tokens: "0" }, ...p.buckets.slice(2)] },
  ]) expect(() => parseOwnerUsageOverview(invalid, filter)).toThrow();
});
