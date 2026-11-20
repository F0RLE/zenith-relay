import { control, record } from "./control";

export const statusStates = {
  operational: "modelStatus.operational", degraded: "modelStatus.degraded",
  unavailable: "modelStatus.unavailable", unknown: "modelStatus.noData",
} as const;
export type StatusState = keyof typeof statusStates;
export type StatusHours = 24 | 168;
export interface StatusMetrics {
  state: StatusState;
  spanMs: number;
  monitoredMs: number;
  operationalMs: number;
  requestMs: number;
  requests: number | null;
  latencyMs: number | null;
  latencySamples: number;
  cacheBps: number | null;
  cacheSamples: number;
}
export interface StatusInterval { startMs: number; endMs: number; metrics?: StatusMetrics }

function integer(v: unknown): number {
  if (typeof v !== "number" || !Number.isSafeInteger(v) || v < 0) throw new Error("Invalid model status");
  return v;
}
const nullable = (v: unknown) => v === null ? null : integer(v);
function metrics(v: unknown, expectedSpan: number): StatusMetrics {
  if (!record(v) || typeof v.state !== "string" || !Object.hasOwn(statusStates, v.state))
    throw new Error("Invalid model status");
  const spanMs = integer(v.span_ms), monitoredMs = integer(v.monitored_ms), operationalMs = integer(v.operational_ms);
  const requestMs = integer(v.request_ms), requests = nullable(v.requests);
  const latencyMs = nullable(v.latency_ms), latencySamples = integer(v.latency_samples);
  const cacheBps = nullable(v.cache_bps), cacheSamples = integer(v.cache_samples);
  const state = v.state as StatusState;
  if (spanMs !== expectedSpan || monitoredMs > spanMs || operationalMs > monitoredMs || requestMs > spanMs
    || (state === "unknown") !== (monitoredMs === 0 || monitoredMs < spanMs)
    || (state === "operational") !== (spanMs > 0 && operationalMs === spanMs)
    || (state === "unavailable" && operationalMs !== 0)
    || (requests === null) !== (requestMs === 0) || (requests ?? 0) > 8192
    || cacheSamples > (requests ?? 0) || (cacheBps === null) !== (cacheSamples === 0) || (cacheBps ?? 0) > 10000
    || latencySamples > (requests ?? 0) || (latencyMs === null) !== (latencySamples === 0))
    throw new Error("Invalid model status coverage");
  return { state, spanMs, monitoredMs, operationalMs, requestMs, requests, latencyMs, latencySamples, cacheBps, cacheSamples };
}

export function parseModelStatus(data: unknown, hours: StatusHours) {
  if (!record(data) || data.hours !== hours || !Array.isArray(data.items) || data.items.length > 64 ||
    !Array.isArray(data.intervals) || data.intervals.length !== (hours === 24 ? 48 : 28))
    throw new Error("Invalid model status window");
  const span = hours * 3_600_000, step = span / data.intervals.length, asOf = integer(data.as_of_ms);
  if (asOf < span || asOf > 8.64e15) throw new Error("Invalid model status anchor");
  const windows = data.intervals.map((row: unknown, i): StatusInterval => {
    const startMs = asOf - span + step * i, endMs = startMs + step;
    if (!record(row) || row.from_ms !== startMs || row.to_ms !== endMs) throw new Error("Invalid model status interval");
    return { startMs, endMs };
  });
  let previous = "";
  const items = data.items.map((row: unknown) => {
    if (!record(row) || typeof row.model !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(row.model) ||
      row.model <= previous || !Array.isArray(row.history) || row.history.length !== windows.length)
      throw new Error("Invalid model status row");
    previous = row.model;
    const history = row.history;
    return { model: row.model, metrics: metrics(row.metrics, span),
      intervals: windows.map((w, i) => ({ ...w, metrics: metrics(history[i], step) })) };
  });
  return { items, summary: metrics(data.summary, span * items.length) };
}

export const readModelStatus = async (hours: StatusHours, signal: AbortSignal) =>
  parseModelStatus(await control(`models/status?hours=${hours}`, { signal }), hours);
export type ModelStatusSnapshot = ReturnType<typeof parseModelStatus>;
