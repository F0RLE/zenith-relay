import { control, record } from "./control";

export const operationJobs = ["request_reconciliation", "usage_report_cleanup", "service_notice_cleanup",
  "checkout_quote_cleanup", "checkout_resume_cleanup", "request_diagnostic_cleanup"] as const;
const states = ["succeeded", "failed", "stale", "unobserved"] as const;
const codes = ["job_failed", "job_recovered"] as const;
const size = 24;
const fail = (): never => { throw new Error("Invalid operations response"); };
const choice = <T extends string>(values: readonly T[], v: unknown): T =>
  typeof v === "string" && values.includes(v as T) ? v as T : fail();
const timestamp = (v: unknown) =>
  typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799999 ? v : fail();
const cursor = (v: unknown) =>
  typeof v === "string" && /^[1-9]\d{0,18}$/.test(v) && BigInt(v) <= 9223372036854775807n ? v : fail();

export function parseOperationsStatus(v: Record<string, unknown>) {
  if (v.version !== 1 || !Array.isArray(v.jobs) || v.jobs.length !== operationJobs.length) return fail();
  const asOf = timestamp(v.as_of_ms);
  const rows = v.jobs.map(row => {
    if (!record(row)) return fail();
    const job = choice(operationJobs, row.job), state = choice(states, row.state);
    const observedAt = row.observed_at_ms === null ? null : timestamp(row.observed_at_ms);
    if ((observedAt === null) !== (state === "unobserved")
      || (observedAt !== null && observedAt > asOf && state !== "stale")) return fail();
    return { job, state, observedAt };
  });
  if (new Set(rows.map(row => row.job)).size !== operationJobs.length) return fail();
  return { asOf, jobs: operationJobs.map(job => rows.find(row => row.job === job)!) };
}

export function parseOperationsEvents(v: Record<string, unknown>, before?: string) {
  if (v.version !== 1 || typeof v.partial !== "boolean" || !Array.isArray(v.items) || v.items.length > size) return fail();
  let previous = before === undefined ? null : BigInt(cursor(before));
  const items = v.items.map(row => {
    if (!record(row)) return fail();
    const id = cursor(row.id), job = choice(operationJobs, row.job), code = choice(codes, row.code);
    const first = timestamp(row.first_at_ms), last = timestamp(row.last_at_ms), count = row.observations;
    if (previous !== null && BigInt(id) >= previous || first > last
      || typeof count !== "number" || !Number.isInteger(count) || count < 1 || count > 2147483647
      || (code === "job_recovered" && (count !== 1 || first !== last))) return fail();
    previous = BigInt(id);
    return { id, job, code, last, count };
  });
  const next = v.next_before === null ? null : cursor(v.next_before);
  if (next !== null && (items.length !== size || next !== items.at(-1)?.id)) return fail();
  return { items, partial: v.partial, next };
}

export async function readOperationsStatus(signal: AbortSignal) {
  return parseOperationsStatus(await control("admin/operations/status", { signal }));
}

export async function readOperationsEvents(before: string | undefined, signal: AbortSignal) {
  const path = before === undefined ? "" : "?before=" + cursor(before);
  return parseOperationsEvents(await control("admin/operations/events" + path, { signal }), before);
}
