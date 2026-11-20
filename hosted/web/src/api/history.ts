import { control, record, uuidText } from "./control";

const fail = (): never => { throw new Error("Invalid history response"); };
const id = (v: unknown) => typeof v === "string" && uuidText.test(v) ? v : fail();
const seconds = (v: unknown) => typeof v === "number" && Number.isSafeInteger(v) && v > 0 && v <= 253402300799 ? v : fail();
const amount = (v: unknown, max=9223372036854775807n) => typeof v === "string" && /^(0|[1-9]\d{0,19})$/.test(v) && BigInt(v) <= max ? BigInt(v) : fail();
const optional = (v: unknown) => v === null ? null : amount(v);
const member = <T extends string>(v: unknown, values: readonly T[]): T => values.includes(v as T) ? v as T : fail();
const states = ["reserved","dispatched","completed","incomplete","failed","uncertain"] as const;
const bases = ["provider_final","provider_partial","not_sent","policy_waiver"] as const;

function readCredit(v: unknown) {
  if (v === null) return null;
  if (!record(v)) return fail();
  const value = amount(v.amount_micro_usd);
  if (value === 0n) return fail();
  return { amount: value, id: id(v.wallet_receipt_id) };
}

export function readReceipt(v: unknown) {
  if (!record(v) || typeof v.model !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(v.model)
    || v.currency !== "USD" || v.precision !== 6) return fail();
  const tariff=amount(v.tariff_revision);
  if (tariff===0n) return fail();
  const input=optional(v.input_tokens), output=optional(v.output_tokens), cached=optional(v.cached_tokens);
  const total=v.total_tokens===null?null:amount(v.total_tokens,18446744073709551614n);
  const charge=optional(v.charge_micro_usd), hold=amount(v.hold_micro_usd);
  const accounting=member(v.accounting,["held","pending","settled","released"] as const);
  const receipt=v.wallet_receipt_id === null ? null : id(v.wallet_receipt_id);
  const basis=v.basis === null ? null : member(v.basis,bases);
  const credit=readCredit(v.credit);
  if ((charge === null) !== (accounting === "held" || accounting === "pending") || (charge === null) !== (receipt === null)
    || (charge === null) !== (basis === null) || (charge !== null && charge>hold)
    || (accounting==="released" && charge!==0n)
    || (accounting==="released") !== (basis==="not_sent" || basis==="policy_waiver")
    || total !== (input===null || output===null ? null : input+output)
    || (credit && (basis !== "provider_partial" || charge === null || credit.amount > charge || credit.id === receipt))
    || (cached !== null && (input === null || cached>input))) return fail();
  return { kind:"usage" as const,id:id(v.request_id),keyId:id(v.key_id),model:v.model,at:seconds(v.started_at_s),status:member(v.execution,states),
    input,output,cached,total,hold,charge,accounting,basis,receipt,tariff,credit };
}
export function readWalletEntry(v: unknown) {
  if (!record(v)) return fail();
  const type=member(v.kind,["payment_credit","request_hold","request_close","request_credit",
    "refund_hold","refund_paid","refund_release"] as const);
  const value=amount(v.amount_micro_usd);
  const refund = type.startsWith("refund_");
  if (type.startsWith("request_") !== (v.request_id !== null) || refund !== (v.refund_id !== null)
    || (type === "request_credit" && value===0n) || (refund && (value === 0n || value % 10000n !== 0n))) return fail();
  return { kind:"wallet" as const,id:id(v.id),at:seconds(v.posted_at_s),type,amount:value,
    requestId:v.request_id===null?null:id(v.request_id), refundId:v.refund_id===null?null:id(v.refund_id) };
}
export async function readWalletHistory(hours: number, filter: { before?: string; id?: string }, signal?:AbortSignal) {
  const query = new URLSearchParams({hours:String(hours),...filter});
  const data=await control("wallet/entries?"+query,signal?{signal}:{});
  if (!Array.isArray(data.items) || data.items.length>32) return fail();
  const items=data.items.map(readWalletEntry);
  const next=data.next_before===null?null:id(data.next_before);
  if (new Set(items.map(i=>i.id)).size!==items.length || (next!==null && next!==items.at(-1)?.id)
    || (filter.id && items.some(i=>i.id!==filter.id))) return fail();
  return {items,next};
}
