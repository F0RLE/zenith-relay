import { expect, test } from "bun:test";
import { parseCaptureDetail, parseCaptureList } from "../../src/api/diagnosticCapture";

const started = 1795130000123;
const expires = started + 60000;
const id = "00000000-0000-4000-8000-000000000001";
const other = "00000000-0000-4000-8000-000000000002";
const summary = {
  id, operation_id: other, request_id: "00000000-0000-4000-8000-000000000003",
  reason: "request_timing", state: "active", audit_pending: false,
  started_at_ms: started, expires_at_ms: expires, ended_at_ms: null,
  limits: { events: 16, bytes: 4096, events_per_second: 16 }, consumed: { events: 2, bytes: 160 },
};
const page = {
  version: 1, as_of_ms: started + 1000, items: [summary],
  config: { default_seconds: 60, max_seconds: 120, max_events: 16, max_bytes: 4096, max_events_per_second: 16 },
};

test("capture list keeps original deadline and caps distinct from current defaults", () => {
  expect(parseCaptureList({ ...page, config: null }).config).toBeNull();
  const parsed = parseCaptureList(page);
  expect(parsed.config).toMatchObject({ duration: 60, max: 120 });
  expect(parsed.items[0]).toMatchObject({ id, expires, events: 2, maxEvents: 16, bytes: 160, maxBytes: 4096 });
  for (const fields of [
    { consumed: { events: 17, bytes: 160 } },
    { consumed: { events: 2, bytes: 4097 } },
    { expires_at_ms: started + 1800001 },
    { state: "stopped", ended_at_ms: null },
    { state: "active", audit_pending: true },
  ]) expect(() => parseCaptureList({ ...page, items: [{ ...summary, ...fields }] })).toThrow();
  expect(() => parseCaptureList({ ...page, items: [summary, { ...summary, id: other }] })).toThrow();
  expect(() => parseCaptureList({ ...page, config: { ...page.config, default_seconds: 121 } })).toThrow();
  expect(parseCaptureList({ ...page, items: [{ ...summary, state: "revoked", audit_pending: true }] }).items[0]?.state)
    .toBe("revoked");
});

test("capture detail accepts only known milestones for the exact capture and time window", () => {
  const first = { sequence: 1, code: "first_stream_frame", occurred_at_ms: started + 1 };
  const next = { sequence: 2, code: "accounting_settled", occurred_at_ms: started + 50 };
  const detail = { version: 1, as_of_ms: page.as_of_ms, session: summary, coverage: "partial", events: [first, next] };
  expect(parseCaptureDetail(detail, id).events.map(row => row.code)).toEqual(["first_stream_frame", "accounting_settled"]);
  expect(parseCaptureDetail({ ...detail, events: [] }, id).events).toEqual([]);
  expect(() => parseCaptureDetail(detail, other)).toThrow();
  expect(() => parseCaptureDetail({ ...detail, coverage: "complete" }, id)).toThrow();
  for (const fields of [
    { sequence: 1 }, { code: first.code }, { code: "unknown" }, { occurred_at_ms: expires },
    { occurred_at_ms: started - 1 },
  ]) expect(() => parseCaptureDetail({ ...detail, events: [first, { ...next, ...fields }] }, id)).toThrow();
});
