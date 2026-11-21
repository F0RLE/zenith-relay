import { expect, test } from "bun:test";
import { ownerUsageHref, parseOwnerUsagePage, parseOwnerUsageView } from "../../src/api/ownerUsage";
import { readCustomerDiagnostic, readOwnerDiagnostic } from "../../src/api/requestDiagnostics";

const request = "00000000-0000-4000-8000-000000000001";
const event = "00000000-0000-4000-8000-000000000002";
const at = 1795130000123;
const observation = {
  diagnostic_version: 1, event_id: event, request_id: request, correlation_id: request, attempt_id: null,
  component: "hosted_executor", sequence: "9007199254740993", code: "upstream_status", stage: "upstream_read",
  origin: null, category: "unknown", severity: "warning", summary_key: "diagnostics.upstreamStatus",
  cause_status: "unknown", occurred_at_ms: at, observed_at_ms: at + 1, expires_at_ms: at + 720 * 3600000,
  evidence: { http_status: 429 },
};

test("owner diagnostic preserves exact identity and never infers origin from HTTP status", () => {
  expect(readOwnerDiagnostic(observation, request)).toMatchObject({
    id: event, status: 429, origin: null, causeUnknown: true, seq: 9007199254740993n,
  });
  expect(readOwnerDiagnostic({ ...observation, summary_key: "fixture-future-key" }, request).summary)
    .toBe("diagnostics.unclassified");
  for (const fields of [
    { diagnostic_version: 0 }, { correlation_id: event }, { event_id: "arbitrary" }, { origin: "http" },
    { stage: "toString" }, { code: "external text" }, { sequence: 10 }, { code: "hosted_terminal_missing" },
    { occurred_at_ms: at + 30002 }, { expires_at_ms: at },
    { evidence: { http_status: 600 } }, { evidence: { http_status: 429, raw: "fixture" } },
  ]) expect(() => readOwnerDiagnostic({ ...observation, ...fields }, request)).toThrow();
});

test("all diagnostic selectors round-trip, and rows must carry the matching observation", () => {
  const view = { errors: true, window: { from: at, to: at + 100 }, stage: "upstream_read", origin: "provider",
    category: "unknown", severity: "warning", code: "upstream_status" };
  const href = new URL(ownerUsageHref(view), "http://localhost:1443");
  expect(parseOwnerUsageView(href.search)).toMatchObject(view);
  for (const q of ["stage=admission", "origin=toString", "category=generic", "severity=notice",
    "code=%27unsafe", "code=" + "x".repeat(65), "code=valid&code=other"])
    expect(parseOwnerUsageView(q)).toBeNull();
  const row = {
    request_id: request, started_at_ms: at, model: "fixture", workspace_id: event, key_id: event,
    source_id: null, execution: "uncertain", accounting: "pending", issues: ["execution"],
    posted_micro_usd: null, diagnostic: observation,
  };
  const response = { version: 1, errors: true, from_ms: at, to_ms: at + 100, as_of_ms: at + 100, next_before: null };
  expect(parseOwnerUsagePage({ ...response, items: [row] }, { ...view, origin: undefined }).items[0]?.diagnostic?.code)
    .toBe("upstream_status");
  expect(() => parseOwnerUsagePage({ ...response, items: [row] }, view)).toThrow();
  expect(() => parseOwnerUsagePage({ ...response, items: [{ ...row, diagnostic: null }] }, view)).toThrow();
});

test("customer decoder accepts only a separate, bounded public explanation", () => {
  const safe = { diagnostic_version: 1, coverage: "partial", code: "relay_outcome_pending",
    summary: "interrupted", next_action: "check_status" };
  expect(readCustomerDiagnostic(safe)).toEqual({ version: 1, coverage: "partial",
    code: "relay_outcome_pending", summary: "diagnostics.publicInterrupted", action: "check_status" });
  expect(readCustomerDiagnostic(null)).toBeNull();
  for (const fields of [
    { attempt_id: event }, { source_id: event }, { evidence: { http_status: 429 } }, { code: "upstream_status" },
    { next_action: "retry_when_allowed" }, { summary: "upstream_failed" }, { coverage: "complete" },
    { summary: "unavailable" }, { code: "relay_not_dispatched" },
  ]) expect(() => readCustomerDiagnostic({ ...safe, ...fields })).toThrow();
});
