import { control, record, uuidText } from "./control";
import { confirmPaymentAction } from "./ownerSession";

const uuid = (v: unknown): v is string => typeof v === "string" && uuidText.test(v);

export function refundAmount(v: unknown) {
  if (typeof v !== "string" || !/^(0|[1-9]\d{0,10})$/.test(v)) throw new Error("Invalid refund amount");
  const n = BigInt(v);
  if (n > 10_000_000_000n || n % 10_000n !== 0n) throw new Error("Invalid refund amount");
  return n;
}

export const refundLabels = {
  requested: "refund.requested", declined: "refund.declined", queued: "refund.queued",
  reconciling: "refund.reconciling", succeeded: "billing.refunded", failed: "refund.failed",
} as const;
type State = keyof typeof refundLabels;
export const refundIsOpen = (state: State) => state === "requested" || state === "queued" || state === "reconciling";

export function readRefund(v: unknown) {
  if (!record(v) || !uuid(v.id) || !uuid(v.payment_id) || typeof v.state !== "string" ||
    !Object.hasOwn(refundLabels, v.state) || typeof v.requested_at_s !== "number" ||
    !Number.isSafeInteger(v.requested_at_s) || v.requested_at_s <= 0 || v.requested_at_s > 253402300799)
    throw new Error("Invalid refund");
  const state = v.state as State, amount = refundAmount(v.amount_micro_usd);
  const posted = state === "succeeded" || state === "failed";
  if (!amount || (posted ? !uuid(v.receipt_id) : v.receipt_id !== null)) throw new Error("Invalid refund receipt");
  return { id: v.id, paymentId: v.payment_id, state, amount, at: v.requested_at_s, receipt: v.receipt_id as string | null };
}

const post = (path: string, csrf: string, body: object) => control(path, {
  method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(body),
});

export async function requestRefund(csrf: string, payment: string, operation: string, amount: bigint) {
  const response = await post(`payments/${payment}/refund-requests`, csrf, {
    operation_id: operation, amount_micro_usd: amount.toString(),
  });
  const result = readRefund(response.refund);
  if (result.paymentId !== payment || result.amount !== amount) throw new Error("Unexpected refund");
  return result;
}

export async function readRefundCases(paymentId: string | null, before?: string, signal?: AbortSignal) {
  const query = new URLSearchParams();
  if (paymentId) query.set("payment_id", paymentId);
  if (before) query.set("before", before);
  const v = await control("admin/payments?" + query, signal ? { signal } : {});
  if (!Array.isArray(v.items) || v.items.length > 32 || (v.next_before !== null && !uuid(v.next_before)))
    throw new Error("Invalid refund page");
  const items = v.items.map(item => {
    if (!record(item) || typeof item.label !== "string" || !item.label || [...item.label].length > 64 || !uuid(item.workspace_id))
      throw new Error("Invalid payment case");
    return { ...readRefund(item), label: item.label, workspace: item.workspace_id };
  });
  if (new Set(items.map(item => item.id)).size !== items.length ||
    (paymentId && items.some(item => item.paymentId !== paymentId)) ||
    (v.next_before !== null && v.next_before !== items.at(-1)?.id)) throw new Error("Invalid refund page");
  return { items, next: v.next_before };
}

export type RefundAction = "approve" | "decline";
export function reviewRefund(csrf: string, id: string, operation: string, action: RefundAction) {
  return confirmPaymentAction("admin/payments/refunds/" + id, csrf, {
    operation_id: operation, action, expected_state: "requested",
  });
}
