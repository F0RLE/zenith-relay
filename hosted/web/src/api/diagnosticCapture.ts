import { control, record, uuidText } from "./control";
import { confirmWithPasskey } from "./ownerSession";

export const captureReasons = ["request_timing", "stream_interruption", "pending_outcome"] as const;
const states = ["active", "stopped", "expired", "exhausted", "revoked"] as const;
const milestones = ["dispatch_committed", "upstream_response", "first_stream_frame", "terminal_observed",
  "evidence_committed", "accounting_settled", "accounting_pending", "request_interrupted"] as const;
const path = "admin/operations/diagnostic-sessions";
const invalid = (): never => { throw new Error("Invalid diagnostic capture response"); };
const object = (v: unknown) => record(v) ? v : invalid();
const uuid = (v: unknown) => typeof v === "string" && uuidText.test(v) ? v : invalid();
const integer = (v: unknown, min: number, max: number) =>
  typeof v === "number" && Number.isInteger(v) && v >= min && v <= max ? v : invalid();
const stamp = (v: unknown) => integer(v, 1, 253402300799999);
const choice = <T extends string>(values: readonly T[], v: unknown): T =>
  typeof v === "string" && values.includes(v as T) ? v as T : invalid();

export type CaptureReason = (typeof captureReasons)[number];
export type CaptureStart = { request_id: string; operation_id: string; reason: CaptureReason; seconds: number };

function parseSummary(raw: unknown) {
  const v = object(raw), limits = object(v.limits), consumed = object(v.consumed);
  const state = choice(states, v.state), started = stamp(v.started_at_ms), expires = stamp(v.expires_at_ms);
  const ended = v.ended_at_ms === null ? null : integer(v.ended_at_ms, started, expires);
  const maxEvents = integer(limits.events, 1, 128), maxBytes = integer(limits.bytes, 128, 32768);
  const events = integer(consumed.events, 0, maxEvents), bytes = integer(consumed.bytes, 0, maxBytes);
  if (expires <= started || expires - started > 1_800_000 || typeof v.audit_pending !== "boolean"
    || (state === "active" && (ended !== null || v.audit_pending))
    || (state !== "active" && ended === null && !(state === "revoked" && v.audit_pending))
    || (events === 0) !== (bytes === 0)) return invalid();
  return { id: uuid(v.id), operation: uuid(v.operation_id), request: uuid(v.request_id), reason: choice(captureReasons, v.reason),
    state, started, expires, ended, maxEvents, maxBytes, events, bytes,
    rate: integer(limits.events_per_second, 1, 16), auditPending: v.audit_pending };
}

export function parseCaptureList(v: Record<string, unknown>) {
  if (v.version !== 1 || !Array.isArray(v.items) || v.items.length > 12) return invalid();
  const items = v.items.map(parseSummary);
  if (new Set(items.map(s => s.id)).size !== items.length
    || items.filter(s => s.state === "active").length > 1) return invalid();
  let config = null;
  if (v.config !== null) {
    const c = object(v.config), max = integer(c.max_seconds, 1, 1800);
    config = { max, duration: integer(c.default_seconds, 1, max),
      events: integer(c.max_events, 1, 128), bytes: integer(c.max_bytes, 128, 32768),
      rate: integer(c.max_events_per_second, 1, 16) };
  }
  return { items, asOf: stamp(v.as_of_ms), config };
}

export function parseCaptureDetail(v: Record<string, unknown>, id: string) {
  const summary = parseSummary(v.session);
  if (v.version !== 1 || summary.id !== id || v.coverage !== "partial"
    || !Array.isArray(v.events) || v.events.length > summary.events) return invalid();
  const events = v.events.map((entry, i) => {
    const row = object(entry);
    return { sequence: integer(row.sequence, i + 1, i + 1), code: choice(milestones, row.code),
      at: integer(row.occurred_at_ms, summary.started, summary.expires - 1) };
  });
  if (new Set(events.map(e => e.code)).size !== events.length) return invalid();
  return { summary, events, asOf: stamp(v.as_of_ms) };
}

export type CapturePage = ReturnType<typeof parseCaptureList>;
export type CaptureDetail = ReturnType<typeof parseCaptureDetail>;

export async function readCaptures(signal: AbortSignal) {
  return parseCaptureList(await control(path, { signal }));
}

export async function readCapture(id: string, signal: AbortSignal) {
  return parseCaptureDetail(await control(`${path}/${uuid(id)}`, { signal }), id);
}

export function startCapture(csrf: string, command: CaptureStart) {
  return confirmWithPasskey(path, csrf, command, path, v => {
    if (!("session" in v)) return undefined;
    const s = parseSummary(v.session);
    if (v.version !== 1 || s.operation !== command.operation_id || s.request !== command.request_id || s.reason !== command.reason
      || s.expires - s.started !== command.seconds * 1000) return invalid();
    return s;
  });
}

export async function stopCapture(csrf: string, id: string) {
  const v = await control(`${path}/${uuid(id)}/stop`, { method: "POST", headers: { "X-Relay-CSRF": csrf } });
  const s = parseSummary(v.session);
  if (v.version !== 1 || s.id !== id || s.state === "active") return invalid();
  return s;
}
