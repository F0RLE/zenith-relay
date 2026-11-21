import { expect, test } from "bun:test";
import { parseOverview, usageLink } from "../../src/api/usageReport";
import { reportView } from "../../src/features/usage/ReportTools";

const report = "db86359e-23f8-4bf8-9dca-989e8f582e3b";
const keyId = "a19e516a-9a68-4e90-a2d5-01ca6fc31880";
const stamp = Date.UTC(2024, 2, 1, 0, 30, 0, 123);
const empty = { requests: 0, token_samples: 0, partial_tokens: 0, total_tokens: "0", spend_micro_usd: "0", unsettled: 0 };
const unknown = { ...empty, requests: 1, total_tokens: null, unsettled: 1 };
const exact = { ...empty, requests: 1, token_samples: 1, total_tokens: "9223372036854775808", spend_micro_usd: "9007199254740993" };
const payload = () => ({
  report_id: report, hours: 24, as_of_ms: stamp, from_ms: stamp - 86400000, to_ms: stamp, model: null, bucket: null,
  key_id: null, execution: null, currency: "USD", precision: 6, coverage: "complete", models: ["fixture"],
  keys: [{ id: keyId, name: "Client", prefix: "rlk_a19e516a9a68" }],
  buckets: [unknown, exact, ...Array.from({ length: 22 }, () => empty)], totals: { ...exact, requests: 2, unsettled: 1 },
});

test("a report preserves exact integer values, separate incomplete counts and UTC bounds", () => {
  const view = parseOverview(payload(), { report });
  expect(view.totals.spend).toBe(9007199254740993n);
  expect(view.totals.tokens).toBe(9223372036854775808n);
  expect(view.buckets.map(b => b.tokens).slice(0, 3)).toEqual([null, 9223372036854775808n, 0n]);
  expect(view.buckets.at(-1)?.to).toBe(stamp);
  expect(view.keys[0]?.id).toBe(keyId);
});

test("mixed snapshots, mismatched aggregates and unsupported coverage fail closed", () => {
  const p = payload();
  for (const v of [
    { ...p, report_id: "other" }, { ...p, as_of_ms: stamp + 1 }, { ...p, coverage: "partial" },
    { ...p, totals: { ...p.totals, spend_micro_usd: "9007199254740992" } },
    { ...p, buckets: [empty, ...p.buckets.slice(1)] }, { ...p, models: ["fixture", "fixture"] },
    { ...p, key_id: keyId }, { ...p, execution: "failed" },
    { ...p, keys: [{ id: keyId, name: "Client", prefix: "rlk_some_other_key" }] },
  ]) expect(() => parseOverview(v, { report })).toThrow();
});

test("a drill-down preserves period, bucket and exact model, including names that resemble a default", () => {
  for (const model of ["all", "name:v1/next"]) {
    const url = new URL(usageLink({ report, bucket: 0, model, keyId, execution: "uncertain" }, 720), "http://localhost");
    expect(reportView(false, url.search)).toEqual({ pinned: report, bucket: 0, model, keyId, execution: "uncertain", hours: 720, id: undefined });
  }
  expect(reportView(false, "?bucket=0")).toBeNull();
  expect(reportView(true, "?id=" + report)).toBeNull();
  expect(reportView(false, "?execution=confirmed")).toBeNull();
  expect(reportView(false, "?model=a&model=b")).toBeNull();
  expect(reportView(false, "?id=" + report + "&key_id=" + keyId)).toBeNull();
});
