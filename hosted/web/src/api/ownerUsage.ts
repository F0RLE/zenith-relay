import { control, record, uuidText } from "./control";
import { isExecution, isHours, isModel, readUsageReceipt, type Execution, type Hours } from "./usageReport";
import { diagnosticChoices, diagnosticCode, readOwnerDiagnostic } from "./requestDiagnostics";

export const accountings = ["held", "pending", "settled", "released"] as const;
const issueCodes = ["execution", "settlement", "usage_evidence", "credit_pending"] as const;
const eventKinds = ["reserved", "dispatch_permitted", "terminal_observed", "terminal_applied",
  "hold_closed", "late_usage", "credit_posted"] as const;
type Accounting = typeof accountings[number];
type Issue = typeof issueCodes[number];
type Bounds = { from: number; to: number };
type Selection = { model?: string | undefined; workspace?: string | undefined; key?: string | undefined;
  source?: string | undefined; execution?: Execution | undefined; accounting?: Accounting | undefined;
  stage?: string | undefined; origin?: string | undefined; category?: string | undefined; severity?: string | undefined; code?: string | undefined };
export type OwnerUsageView = Selection & { errors: boolean; window: { hours: Hours } | Bounds };
export const defaultOwnerUsage: OwnerUsageView = { errors: false, window: { hours: 168 } };

const fail = (): never => { throw new Error("Invalid owner usage response"); };
const validTimestamp = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799999;
const timestamp = (v: unknown) => validTimestamp(v) ? v : fail();
const hexId = (v: unknown): v is string => typeof v === "string" && uuidText.test(v);
const id = (v: unknown) => hexId(v) ? v : fail();
const optionalId = (v: unknown) => v === null ? null : id(v);
const amount = (v: unknown, max = 9223372036854775807n) =>
  typeof v === "string" && /^(0|[1-9][0-9]{0,39})$/.test(v) && BigInt(v) <= max ? BigInt(v) : fail();
const member = <T extends string>(v: unknown, values: readonly T[]): T => values.includes(v as T) ? v as T : fail();
export const isAccounting = (v: unknown): v is Accounting => accountings.includes(v as Accounting);

const diagnosticFields = ["stage", "origin", "category", "severity", "code"] as const;
const fields = ["model", "workspace_id", "key_id", "source_id", "execution", "accounting", ...diagnosticFields] as const;
export const hasDiagnosticSelection = (view: Selection) => diagnosticFields.some(field => view[field] !== undefined);
function decimalMs(v: string | null) {
  return v !== null && /^[1-9]\d{0,14}$/.test(v) && validTimestamp(Number(v)) ? Number(v) : null;
}

export function parseOwnerUsageView(query: string): OwnerUsageView | null {
  const q = new URLSearchParams(query);
  const allow = new Set<string>([...fields, "tab", "hours", "from_ms", "to_ms"]);
  if (query.length > 1024 || [...q].some(([k]) => !allow.has(k) || q.getAll(k).length !== 1)) return null;
  const tab = q.get("tab"), hours = q.get("hours");
  if (tab !== null && tab !== "errors" && tab !== "requests") return null;
  const [model, workspace, key, source, execution, accounting, stage, origin, category, severity, code] = fields.map(k => q.get(k) ?? undefined);
  if ((model !== undefined && !isModel(model)) || [workspace, key, source].some(v => v !== undefined && !hexId(v))
    || (execution !== undefined && !isExecution(execution)) || (accounting !== undefined && !isAccounting(accounting))
    || (code !== undefined && !diagnosticCode.test(code))
    || (Object.keys(diagnosticChoices) as (keyof typeof diagnosticChoices)[]).some(field => {
      const value = q.get(field);
      return value !== null && !Object.hasOwn(diagnosticChoices[field], value);
    })) return null;
  let window: OwnerUsageView["window"];
  if (q.has("from_ms") || q.has("to_ms")) {
    const from = decimalMs(q.get("from_ms")), to = decimalMs(q.get("to_ms"));
    if (hours !== null || from === null || to === null || from >= to) return null;
    window = { from, to };
  } else {
    const number = hours === null ? 168 : /^\d{2,3}$/.test(hours) ? Number(hours) : 0;
    if (!isHours(number)) return null;
    window = { hours: number };
  }
  return { errors: tab === "errors", window, model, workspace, key, source, stage, origin, category, severity, code,
    execution: isExecution(execution) ? execution : undefined, accounting: isAccounting(accounting) ? accounting : undefined };
}

function params(view: OwnerUsageView, window = view.window) {
  const p = new URLSearchParams("hours" in window ? { hours: String(window.hours) }
    : { from_ms: String(window.from), to_ms: String(window.to) });
  const values = [view.model, view.workspace, view.key, view.source, view.execution, view.accounting,
    ...diagnosticFields.map(field => view[field])];
  fields.forEach((name, i) => { const value = values[i]; if (value !== undefined) p.set(name, value); });
  return p;
}

export function ownerUsageHref(view: OwnerUsageView, requestId?: string) {
  const p = params(view);
  if (view.errors) p.set("tab", "errors");
  return "/admin/usage" + (requestId ? "/requests/" + requestId : "") + "?" + p;
}

export function ownerRequestPath(path: string) {
  const prefix = "/admin/usage/requests/";
  const candidate = path.startsWith(prefix) ? path.slice(prefix.length) : null;
  return hexId(candidate) ? candidate : null;
}

function issues(v: unknown, execution: string, accounting: Accounting) {
  if (!Array.isArray(v) || v.length > issueCodes.length) return fail();
  const parsed = v.map(code => member(code, issueCodes));
  if (new Set(parsed).size !== parsed.length
    || parsed.includes("execution") !== ["failed", "incomplete", "uncertain"].includes(execution)
    || parsed.includes("settlement") !== (execution === "completed" && ["held", "pending"].includes(accounting))) return fail();
  return parsed;
}

function row(v: unknown) {
  if (!record(v) || !isExecution(v.execution) || !isAccounting(v.accounting) || !isModel(v.model)) return fail();
  const posted = v.posted_micro_usd === null ? null : amount(v.posted_micro_usd);
  if ((posted === null) !== ["held", "pending"].includes(v.accounting) || (v.accounting === "released" && posted !== 0n)) return fail();
  const request = id(v.request_id);
  return { id: request, at: timestamp(v.started_at_ms), model: v.model, execution: v.execution,
    accounting: v.accounting, issues: issues(v.issues, v.execution, v.accounting), posted,
    diagnostic: v.diagnostic == null ? null : readOwnerDiagnostic(v.diagnostic, request),
    workspace: id(v.workspace_id), key: id(v.key_id), source: optionalId(v.source_id) };
}

export function parseOwnerUsagePage(v: Record<string, unknown>, view: OwnerUsageView, window = view.window, before?: string) {
  const from = timestamp(v.from_ms), to = timestamp(v.to_ms), asOf = timestamp(v.as_of_ms);
  if (v.version !== 1 || v.errors !== view.errors || from >= to || to > asOf
    || ("hours" in window ? to !== asOf || to - from !== window.hours * 3600000 : from !== window.from || to !== window.to)
    || !Array.isArray(v.items) || v.items.length > 32) return fail();
  const items = v.items.map(row), next = optionalId(v.next_before);
  if ((next !== null && (items.length !== 32 || next !== items.at(-1)?.id))
    || new Set(items.map(i => i.id)).size !== items.length
    || items.some((i, n) => i.at < from || i.at >= to || i.id === before || (n > 0 && i.at > items[n - 1]!.at)
      || (view.model !== undefined && i.model !== view.model) || (view.workspace !== undefined && i.workspace !== view.workspace)
      || (view.key !== undefined && i.key !== view.key) || (view.source !== undefined && i.source !== view.source)
      || (view.execution !== undefined && i.execution !== view.execution)
      || (view.accounting !== undefined && i.accounting !== view.accounting) || (view.errors && !i.issues.length)
      || diagnosticFields.some(field => view[field] !== undefined && i.diagnostic?.[field] !== view[field]))) return fail();
  return { items, next, from, to, asOf };
}

function readAttempt(v: unknown) {
  if (v === null) return null;
  if (!record(v)) return fail();
  const source = optionalId(v.source_id);
  const revision = v.purchase_revision === null ? null : amount(v.purchase_revision);
  const value = v.valued_micro_usd === null ? null : amount(v.valued_micro_usd, 10n ** 40n - 1n);
  const terminal = v.terminal === null ? null : member(v.terminal, ["completed", "incomplete", "failed"] as const);
  if ((revision === null) !== (source === null) || revision === 0n || (value !== null && source === null)) return fail();
  return { id: id(v.id), source, revision, value, terminal };
}

export function parseOwnerRequest(v: Record<string, unknown>, target: string) {
  if (v.version !== 1 || typeof v.streaming !== "boolean" || !Array.isArray(v.timeline)
    || v.timeline.length > eventKinds.length) return fail();
  const receipt = readUsageReceipt(v.receipt), attempt = readAttempt(v.attempt);
  const attention: Issue[] = issues(v.issues, receipt.status, receipt.accounting);
  const timeline = v.timeline.map(e => {
    if (!record(e)) return fail();
    return { event: member(e.event, eventKinds), at: timestamp(e.at_ms) };
  });
  const events = new Map(timeline.map(e => [e.event, e.at]));
  if (receipt.id !== target || events.size !== timeline.length || events.get("reserved") !== receipt.atMs
    || events.has("dispatch_permitted") !== !!attempt || events.has("terminal_observed") !== !!attempt?.terminal
    || (events.has("terminal_applied") && !events.has("terminal_observed")) || events.has("hold_closed") !== (receipt.charge !== null)
    || events.has("credit_posted") !== !!receipt.credit
    || (attention.includes("credit_pending") && (receipt.basis !== "provider_partial" || !!receipt.credit || !events.has("late_usage")))
    || timeline.some((event, i) => i > 0 && event.at < timeline[i - 1]!.at)) return fail();
  const diagnostics = v.diagnostics === undefined ? [] : Array.isArray(v.diagnostics) && v.diagnostics.length <= 32
    ? v.diagnostics.map(e => readOwnerDiagnostic(e, target)) : fail();
  const coverage = diagnostics.length ? "partial" : "unavailable";
  if ((v.diagnostic_coverage !== undefined && v.diagnostic_coverage !== coverage)
    || new Set(diagnostics.map(e => e.id)).size !== diagnostics.length
    || new Set(diagnostics.map(e => e.code)).size !== diagnostics.length
    || diagnostics.some((e, i) => (e.attempt !== null && e.attempt !== attempt?.id)
      || (i > 0 && e.seq <= diagnostics[i - 1]!.seq))) return fail();
  return { receipt, attempt, attention, timeline, diagnostics, coverage, asOf: timestamp(v.as_of_ms), workspace: id(v.workspace_id), streaming: v.streaming };
}

export async function readOwnerUsagePage(view: OwnerUsageView, window: OwnerUsageView["window"], before: string | undefined, signal: AbortSignal) {
  const q = params(view, window);
  if (before) q.set("before", before);
  const route = view.errors ? "errors" : "requests";
  return parseOwnerUsagePage(await control(`admin/usage/${route}?` + q, { signal }), view, window, before);
}

export async function readOwnerRequest(target: string, signal: AbortSignal) {
  return parseOwnerRequest(await control(`admin/usage/requests/${target}`, { signal }), target);
}
