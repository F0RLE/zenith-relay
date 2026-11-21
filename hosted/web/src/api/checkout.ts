import { control, record, uuidText } from "./control";
import { readRefund, refundAmount } from "./refunds";

const usdCent = 10_000n;
const uuid = (v: unknown): v is string => typeof v === "string" && uuidText.test(v);
const time = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799;
const label = (v: unknown): v is string => typeof v === "string" && v.trim() === v && [...v].length > 0 && [...v].length <= 64;
const units = (v: unknown, max: bigint) => {
  if (typeof v !== "string" || !/^(0|[1-9]\d{0,10})$/.test(v)) throw new Error("Invalid checkout amount");
  const n = BigInt(v);
  if (n > max) throw new Error("Invalid checkout amount");
  return n;
};
const credit = (v: unknown) => {
  const n = units(v, 10_000_000_000n);
  if (n === 0n || n % usdCent !== 0n) throw new Error("Invalid wallet credit");
  return n;
};
const payable = (v: unknown) => units(v, 2_000_000n) * usdCent;

export function parseCreditInput(text: string) {
  const match = /^(\d{1,5})(?:[.,](\d{0,2}))?$/.exec(text.trim());
  return match ? BigInt(match[1]!) * 1_000_000n + BigInt((match[2] ?? "").padEnd(2, "0")) * usdCent : null;
}

function method(v: unknown) {
  if (!record(v) || !uuid(v.id) || !label(v.label) || !Array.isArray(v.quick_credit_micro_usd) ||
    v.quick_credit_micro_usd.length > 8 || typeof v.custom_amount !== "boolean") throw new Error("Invalid payment method");
  const min = credit(v.min_credit_micro_usd), max = credit(v.max_credit_micro_usd);
  const quick = v.quick_credit_micro_usd.map(credit);
  if (min > max || new Set(quick).size !== quick.length || quick.some(n => n < min || n > max) ||
    (!v.custom_amount && !quick.length)) throw new Error("Invalid credit policy");
  return { id: v.id, label: v.label, min, max, quick, custom: v.custom_amount };
}

export async function readCheckoutMethods(signal?: AbortSignal) {
  const v = await control("payments/methods", signal ? { signal } : {});
  if (!Array.isArray(v.items) || v.items.length > 16) throw new Error("Invalid checkout methods");
  const items = v.items.map(method);
  if (new Set(items.map(v => v.id)).size !== items.length) throw new Error("Duplicate payment method");
  return items;
}

export type CheckoutMethod = Awaited<ReturnType<typeof readCheckoutMethods>>[number];

function post(path: string, csrf: string, body?: object) {
  return control(path, { method: "POST", headers: { "X-Relay-CSRF": csrf,
    ...(body && { "Content-Type": "application/json" }) }, ...(body && { body: JSON.stringify(body) }) });
}
export async function quoteTopUp(csrf: string, method: string, amount: bigint) {
  const result = await post("payments/quotes", csrf, { method_id: method, credit_micro_usd: amount.toString() });
  const v = result.quote;
  if (!record(v) || !uuid(v.id) || v.method_id !== method || v.currency !== "USD" || !time(v.expires_at_s) ||
    credit(v.credit_micro_usd) !== amount) throw new Error("Unexpected checkout quote");
  const fee = payable(v.fee_minor), total = payable(v.payable_minor);
  if (total !== amount + fee) throw new Error("Invalid checkout total");
  return { id: v.id, credit: amount, fee, total, expiresAt: v.expires_at_s };
}

export type CheckoutQuote = Awaited<ReturnType<typeof quoteTopUp>>;

type PaymentState = "processing" | "awaiting_payment" | "paid" | "closed";
function payment(v: unknown) {
  if (!record(v) || !uuid(v.id) || !uuid(v.operation_id) || !uuid(v.quote_id) || !label(v.label) ||
    !time(v.created_at_s) || v.currency !== "USD" || (v.state !== "processing" && v.state !== "awaiting_payment" &&
    v.state !== "paid" && v.state !== "closed") || (v.state === "paid" ? !uuid(v.receipt_id) : v.receipt_id !== null) ||
    typeof v.can_resume !== "boolean" || (v.can_resume && v.state !== "awaiting_payment"))
    throw new Error("Invalid payment");
  const amount = credit(v.credit_micro_usd), total = payable(v.payable_minor);
  const refundable = refundAmount(v.refundable_micro_usd);
  const refund = v.refund === null ? null : readRefund(v.refund);
  if (total < amount || refundable > amount ||
    (v.state !== "paid" && (refund !== null || refundable !== 0n)) ||
    (refund && (refund.paymentId !== v.id || refund.amount > amount ||
      (["queued", "reconciling", "succeeded"].includes(refund.state) && refund.amount + refundable > amount))))
    throw new Error("Invalid payment total");
  return { id: v.id, operation: v.operation_id, quote: v.quote_id, label: v.label, amount, total,
    state: v.state as PaymentState, receipt: v.receipt_id as string | null, at: v.created_at_s, canResume: v.can_resume,
    refundable, refund };
}
export type TopUp = ReturnType<typeof payment>;

export async function readTopUp(id: string) {
  const item = payment((await control("payments/" + id)).payment);
  if (item.id !== id) throw new Error("Unexpected payment");
  return item;
}

export async function readTopUps(filter?: { operation_id: string } | { before: string }, signal?: AbortSignal) {
  const v = await control("payments" + (filter ? "?" + new URLSearchParams(filter) : ""), signal ? { signal } : {});
  if (!Array.isArray(v.items) || v.items.length > 32 || v.next_before !== null && !uuid(v.next_before))
    throw new Error("Invalid payment list");
  const items = v.items.map(payment);
  if (new Set(items.map(i => i.id)).size !== items.length ||
    (v.next_before !== null && v.next_before !== items.at(-1)?.id) ||
    (filter && "operation_id" in filter && (items.length > 1 || items.some(i => i.operation !== filter.operation_id))))
    throw new Error("Invalid payment page");
  return { items, next: v.next_before };
}

export async function confirmTopUp(csrf: string, quote: string, operation: string) {
  const result = payment((await post("payments", csrf, { quote_id: quote, operation_id: operation })).payment);
  if (result.quote !== quote || result.operation !== operation) throw new Error("Unexpected payment confirmation");
  return result;
}

export function parseResumedTopUp(data: Record<string, unknown>, original: TopUp) {
  const current = payment(data.payment);
  if (current.id !== original.id || current.operation !== original.operation || current.quote !== original.quote ||
    current.amount !== original.amount || current.total !== original.total) throw new Error("Unexpected invoice");
  if (data.handoff_url === null) return { payment: current, handoff: null };
  if (!current.canResume || typeof data.handoff_url !== "string" || data.handoff_url.length > 2048 ||
    !data.handoff_url.startsWith("https://") || data.handoff_url.includes("#")) throw new Error("Invalid checkout destination");
  const target = new URL(data.handoff_url);
  if (target.username || target.password || target.href !== data.handoff_url)
    throw new Error("Invalid checkout destination");
  return { payment: current, handoff: target.href };
}

export async function resumeTopUp(csrf: string, original: TopUp) {
  return parseResumedTopUp(await post("payments/" + original.id + "/resume", csrf), original);
}
