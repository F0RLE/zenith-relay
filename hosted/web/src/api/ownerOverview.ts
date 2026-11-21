import { control, record } from "./control";
import { isHours, isModel, type Hours } from "./usageReport";

export type OwnerHours = Hours | 0;
export type OwnerOverviewFilter = { hours: OwnerHours; model?: string };
const hourMs = 3600000;
const maxModels = 256;
const metricKeys = ["requests", "samples", "partial", "unsettled", "spend"] as const;
const fail = (): never => { throw new Error("Invalid owner usage overview"); };
const timestamp = (v: unknown) =>
  typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799999 ? v : fail();
const integer = (v: unknown, digits = 19) =>
  typeof v === "string" && v.length <= digits && /^(0|[1-9][0-9]*)$/.test(v) ? BigInt(v) : fail();

function metrics(v: unknown) {
  if (!record(v)) return fail();
  const requests = integer(v.requests), samples = integer(v.token_samples), partial = integer(v.partial_tokens),
    unsettled = integer(v.unsettled), tokens = v.total_tokens === null ? null : integer(v.total_tokens, 39),
    spend = integer(v.spend_micro_usd, 39);
  if (samples > requests || partial > requests || unsettled > requests
    || (tokens === null) !== (requests > 0n && samples === 0n)
    || (requests === 0n && (tokens !== 0n || spend !== 0n))) return fail();
  return { requests, samples, partial, unsettled, tokens, spend };
}

export function parseOwnerUsageOverview(v: Record<string, unknown>, { hours, model }: OwnerOverviewFilter) {
  if ((hours !== 0 && !isHours(hours)) || v.hours !== hours || v.model !== (model ?? null)
    || v.currency !== "USD" || v.precision !== 6 || typeof v.more_models !== "boolean") return fail();
  const from = timestamp(v.from_ms), to = timestamp(v.as_of_ms), span = to - from;
  const step = hours === 0 ? Math.ceil(span / 30) : hours === 24 ? hourMs : 24 * hourMs;
  if (span <= 0 || (hours !== 0 && span !== hours * hourMs) || v.step_ms !== step
    || !Array.isArray(v.buckets) || v.buckets.length !== Math.ceil(span / step)
    || !Array.isArray(v.models) || v.models.length > maxModels || (v.more_models && v.models.length !== maxModels)) return fail();
  const models = v.models.map(m => isModel(m) ? m : fail());
  if (models.some((m, i) => i > 0 && models[i - 1]! >= m)) return fail();
  const totals = metrics(v.totals);
  const buckets = v.buckets.map((b, i) => ({
    ...metrics(b), from: from + i * step, to: Math.min(from + (i + 1) * step, to),
  }));
  if (metricKeys.some(key => buckets.reduce((sum, b) => sum + b[key], 0n) !== totals[key])
    || buckets.reduce((sum, b) => sum + (b.tokens ?? 0n), 0n) !== (totals.tokens ?? 0n)) return fail();
  return { from, to, totals, buckets, models, moreModels: v.more_models };
}

export async function readOwnerUsageOverview(filter: OwnerOverviewFilter, signal: AbortSignal) {
  if (filter.model !== undefined && !isModel(filter.model)) return fail();
  const search = new URLSearchParams({ hours: String(filter.hours), ...(filter.model ? { model: filter.model } : {}) });
  return parseOwnerUsageOverview(await control("admin/overview/usage?" + search, { signal }), filter);
}
