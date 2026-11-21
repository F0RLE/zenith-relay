import { control, controlResponse, record, uuidText } from "./control";
import { readReceipt } from "./history";
import { readCustomerDiagnostic } from "./requestDiagnostics";

export type Hours = 24 | 168 | 720;
export const executions = ["reserved", "dispatched", "completed", "incomplete", "failed", "uncertain"] as const;
export type Execution = typeof executions[number];
export const isExecution = (v: unknown): v is Execution => executions.includes(v as Execution);
export type UsageSelection = { model?: string | undefined; bucket?: number | undefined; keyId?: string | undefined;
  execution?: Execution | undefined };
export type UsageFilter = UsageSelection & { report: string };
export const reportKeyPrefix = (id: string) => "rlk_" + id.replaceAll("-", "").slice(0, 12);
export const isModel = (s: unknown): s is string =>
  typeof s === "string" && /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(s);
export const isHours = (n: unknown): n is Hours => n === 24 || n === 168 || n === 720;
const widths = { 24: 24, 168: 7, 720: 30 };
const maxRows = 8192;
const hourMs = 3600000;
const fail = (): never => { throw new Error("Invalid usage report"); };
const timestamp = (v: unknown) => typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799999 ? v : fail();
const counter = (v: unknown, max = maxRows) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= max ? v : fail();
const integer = (v: unknown) => typeof v === "string" && /^(0|[1-9]\d{0,23})$/.test(v) ? BigInt(v) : fail();

function metrics(v: unknown) {
  if (!record(v)) return fail();
  const requests = counter(v.requests);
  const samples = counter(v.token_samples, requests);
  const partial = counter(v.partial_tokens, requests);
  const unsettled = counter(v.unsettled, requests);
  const tokens = v.total_tokens === null ? null : integer(v.total_tokens);
  const spend = integer(v.spend_micro_usd);
  if ((tokens === null) !== (requests > 0 && samples === 0) || (!requests && (tokens !== 0n || spend !== 0n))) return fail();
  return { requests, tokens, spend, samples, partial, unsettled };
}

function bounds(v: Record<string, unknown>, filter: UsageFilter) {
  if (v.report_id !== filter.report || v.model !== (filter.model ?? null) || v.bucket !== (filter.bucket ?? null)
    || v.key_id !== (filter.keyId ?? null) || v.execution !== (filter.execution ?? null)
    || v.coverage !== "complete" || v.currency !== "USD" || v.precision !== 6 || !isHours(v.hours)) return fail();
  const asOf = timestamp(v.as_of_ms), from = timestamp(v.from_ms), to = timestamp(v.to_ms);
  const hours = v.hours, count = widths[hours], step = hours * hourMs / count;
  if (filter.bucket !== undefined && (counter(filter.bucket, count - 1) !== filter.bucket)) return fail();
  const start = asOf - hours * hourMs;
  if (from !== (filter.bucket === undefined ? start : start + filter.bucket * step)
    || to !== (filter.bucket === undefined ? asOf : from + step)) return fail();
  return { asOf, from, to, hours, step, count, totals: metrics(v.totals) };
}

export function parseOverview(v: Record<string, unknown>, filter: UsageFilter) {
  const window = bounds(v, filter);
  if (filter.bucket !== undefined || !Array.isArray(v.buckets) || v.buckets.length !== window.count
    || !Array.isArray(v.models) || v.models.length > maxRows || !Array.isArray(v.keys) || v.keys.length > maxRows) return fail();
  const models = v.models.map(m => isModel(m) ? m : fail());
  const keys = v.keys.map(k => {
    if (!record(k) || typeof k.id !== "string" || !uuidText.test(k.id) || k.prefix !== reportKeyPrefix(k.id)
      || typeof k.name !== "string" || !k.name || k.name.length > 160 || /[\x00-\x1f\x7f]/.test(k.name)) return fail();
    return { id: k.id, name: k.name, prefix: k.prefix };
  });
  if (new Set(models).size !== models.length || new Set(keys.map(k => k.id)).size !== keys.length) return fail();
  const buckets = v.buckets.map((b, i) => ({ ...metrics(b), from: window.from + window.step * i, to: window.from + window.step * (i + 1) }));
  const sum = <T extends "requests" | "samples" | "partial" | "unsettled">(key: T) => buckets.reduce((a, b) => a + b[key], 0);
  if (sum("requests") !== window.totals.requests || sum("samples") !== window.totals.samples
    || sum("partial") !== window.totals.partial || sum("unsettled") !== window.totals.unsettled
    || buckets.reduce((a, b) => a + b.spend, 0n) !== window.totals.spend
    || buckets.reduce((a, b) => a + (b.tokens ?? 0n), 0n) !== (window.totals.tokens ?? 0n)) return fail();
  return { ...window, buckets, models, keys };
}

export function readUsageReceipt(v: unknown) {
  if (!record(v)) return fail();
  const base = readReceipt(v);
  const atMs = timestamp(v.started_at_ms);
  const quality = v.usage_quality;
  if ((quality !== "final" && quality !== "partial" && quality !== "unknown") || Math.floor(atMs / 1000) !== base.at
    || (quality === "final" && base.total === null)
    || (quality === "unknown") !== (base.input === null && base.output === null)) return fail();
  return { ...base, atMs, quality };
}

const params = (filter: UsageSelection & { report?: string | undefined }) => new URLSearchParams({
  ...(filter.report !== undefined ? { report: filter.report } : {}),
  ...(filter.model !== undefined ? { model: filter.model } : {}),
  ...(filter.bucket !== undefined ? { bucket: String(filter.bucket) } : {}),
  ...(filter.keyId !== undefined ? { key_id: filter.keyId } : {}),
  ...(filter.execution !== undefined ? { execution: filter.execution } : {}),
});

export async function startUsageReport(hours: Hours, csrf: string, signal: AbortSignal) {
  const v = await control("usage/reports", { method: "POST", headers: {
    "Content-Type": "application/json", "X-Relay-CSRF": csrf,
  }, body: JSON.stringify({ hours }), signal });
  return typeof v.report_id === "string" && uuidText.test(v.report_id) ? v.report_id : fail();
}

export async function readUsageOverview(filter: UsageFilter, signal: AbortSignal) {
  const q = filter.model === undefined ? "" : "?" + new URLSearchParams({ model: filter.model });
  return parseOverview(await control("usage/reports/" + filter.report + q, { signal }), filter);
}

export async function readUsagePage(filter: UsageFilter | undefined, before: string | undefined, id: string | undefined, signal: AbortSignal) {
  const q = filter ? params(filter) : new URLSearchParams();
  if (before) q.set("before", before);
  if (id) q.set("id", id);
  const v = await control("usage/requests?" + q, { signal });
  const window = filter ? bounds(v, filter) : null;
  if (!Array.isArray(v.items) || v.items.length > 32) return fail();
  const items = v.items.map(readUsageReceipt);
  const next = v.next_before;
  if ((next !== null && (!filter || typeof next !== "string" || next.length !== 101
      || !/^[0-9a-f]{64}$/.test(next.slice(37)) || next.slice(0, 37) !== items.at(-1)?.id + "." || items.length !== 32))
    || new Set(items.map(i => i.id)).size !== items.length
    || (id && items.some(i => i.id !== id))
    || (window && filter && (items.length > window.totals.requests || items.some(i =>
      i.atMs < window.from || i.atMs >= window.to || (filter.model !== undefined && i.model !== filter.model)
      || (filter.keyId !== undefined && i.keyId !== filter.keyId)
      || (filter.execution !== undefined && i.status !== filter.execution))))) return fail();
  const diagnostic = readCustomerDiagnostic(v.diagnostic);
  if (diagnostic && (!id || filter || items.length !== 1
    || (diagnostic.action === "check_status") !== (items[0]?.charge === null))) return fail();
  return { items, next, window, diagnostic };
}

export function usageLink(filter: UsageSelection & { report?: string | undefined }, hours: Hours) {
  const q = params(filter);
  q.set("hours", String(hours));
  return "/app/usage?" + q;
}

export async function downloadUsageReport({ report, ...selection }: UsageFilter, signal: AbortSignal) {
  if (!uuidText.test(report)) return fail();
  const r = await controlResponse(`usage/reports/${report}/csv?` + params(selection), { signal });
  if (r.headers.get("content-type")?.split(";")[0] !== "text/csv") return fail();
  const data = await r.blob();
  if (!data.size || data.size > 8 * 1024 * 1024) return fail();
  return data;
}
