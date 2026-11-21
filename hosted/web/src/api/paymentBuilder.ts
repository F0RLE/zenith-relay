import { parseCreditInput } from "./checkout";
import { confirmPaymentAction } from "./ownerSession";
import { control, record, uuidText } from "./control";

const path = "admin/payment-methods";
const uuid = (v: unknown): v is string => typeof v === "string" && uuidText.test(v);
const integer = (v: unknown, min: number, max: number): v is number =>
  typeof v === "number" && Number.isInteger(v) && v >= min && v <= max;
const label = (v: unknown): v is string => typeof v === "string" && v.trim() === v && [...v].length >= 1 && [...v].length <= 64;
const code = (v: unknown): v is string => typeof v === "string" && /^[a-z0-9][a-z0-9_-]{0,63}$/.test(v);

type FeePayer = "merchant" | "customer";
const payer = (v: unknown): v is FeePayer => v === "merchant" || v === "customer";

type Selection = { binding_id: string; code: string; fee_payer: FeePayer };
export type CheckoutDraft = {
  connections: string[]; methods: Selection[]; minimum_credit_minor: number;
  quick_amounts: number[]; custom_amount: boolean; open_limit: number;
};

export const methodKey = (binding: string, code: string) => `${binding}/${code}`;

function validAmounts(min: number, quick: number[], custom: boolean) {
  return integer(min, 1, 1_000_000) && quick.length <= 8 && (custom || quick.length > 0)
    && new Set(quick).size === quick.length && quick.every(n => integer(n, min, 1_000_000));
}

export function checkoutSettings(minimum: string, presets: string, custom: boolean, limit: string) {
  const minor = (text: string) => Number((parseCreditInput(text) ?? -10_000n) / 10_000n);
  const min = minor(minimum), quick = presets.trim().split(/[\s;]+/).filter(Boolean).map(minor);
  return validAmounts(min, quick, custom) && /^(?:[1-9]|10)$/.test(limit)
    ? { minimum_credit_minor: min, quick_amounts: quick, custom_amount: custom, open_limit: Number(limit) }
    : null;
}

function selection(v: unknown): v is Selection {
  return record(v) && uuid(v.binding_id) && code(v.code) && payer(v.fee_payer);
}

function draft(v: unknown): CheckoutDraft {
  if (!record(v) || !integer(v.minimum_credit_minor, 1, 1_000_000) || !Array.isArray(v.quick_amounts)
    || typeof v.custom_amount !== "boolean" || !integer(v.open_limit, 1, 10)
    || !validAmounts(v.minimum_credit_minor, v.quick_amounts, v.custom_amount)
    || !Array.isArray(v.connections) || v.connections.length > 8 || !v.connections.every(uuid)
    || new Set(v.connections).size !== v.connections.length
    || !Array.isArray(v.methods) || v.methods.length > 16 || !v.methods.every(selection)
    || new Set(v.methods.map(m => methodKey(m.binding_id, m.code))).size !== v.methods.length
    || !v.methods.every(m => (v.connections as string[]).includes(m.binding_id))) throw new Error("Invalid checkout draft");
  return { connections: v.connections, methods: v.methods, minimum_credit_minor: v.minimum_credit_minor,
    quick_amounts: v.quick_amounts, custom_amount: v.custom_amount, open_limit: v.open_limit };
}

function method(v: unknown) {
  if (!record(v) || !code(v.code) || !label(v.label) || !integer(v.fee_minor, 0, 1_000_000)
    || typeof v.merchant_pays !== "boolean" || typeof v.customer_pays !== "boolean"
    || (!v.merchant_pays && !v.customer_pays)) throw new Error("Invalid adapter method");
  return { code: v.code, label: v.label, fee: v.fee_minor, merchant: v.merchant_pays, customer: v.customer_pays };
}

function adapter(v: unknown) {
  if (!record(v) || !uuid(v.id) || !label(v.label) || !Array.isArray(v.methods)
    || !integer(v.methods.length, 1, 16)) throw new Error("Invalid adapter");
  const methods = v.methods.map(method);
  if (new Set(methods.map(m => m.code)).size !== methods.length) throw new Error("Duplicate adapter method");
  return { id: v.id, label: v.label, methods };
}

function published(v: unknown) {
  if (!record(v) || !uuid(v.id) || !label(v.label) || !payer(v.fee_payer)
    || !integer(v.processing_fee_minor, 0, 1_000_000)) throw new Error("Invalid published method");
  return { id: v.id, label: v.label, payer: v.fee_payer, fee: v.processing_fee_minor };
}

export async function readPaymentBuilder(signal?: AbortSignal) {
  const v = await control(path, signal ? { signal } : {});
  if (typeof v.editable !== "boolean" || typeof v.revision !== "string" || !/^[1-9]\d{0,18}$/.test(v.revision)
    || !Array.isArray(v.adapters) || !Array.isArray(v.published) || v.published.length > 16
    || !Array.isArray(v.methods_without_amounts) || v.methods_without_amounts.length > 16
    || (v.eligible_count !== null && !integer(v.eligible_count, 0, 16))
    || (v.checked_until_s !== null && !integer(v.checked_until_s, 1, 253402300799))
    || !integer(v.published_open_limit, 1, 10)) throw new Error("Invalid checkout configuration");
  const saved = draft(v.draft);
  const noAmounts = v.methods_without_amounts.map(m => {
    if (!record(m) || !uuid(m.binding_id) || !code(m.code) ||
      !saved.methods.some(s => s.binding_id === m.binding_id && s.code === m.code)) throw new Error("Invalid omitted method");
    return methodKey(m.binding_id, m.code);
  });
  return { editable: v.editable, revision: v.revision, draft: saved, adapters: v.adapters.map(adapter), noAmounts,
    published: v.published.map(published), eligibleCount: v.eligible_count, checkedUntil: v.checked_until_s,
    openLimit: v.published_open_limit };
}

export type PaymentBuilder = Awaited<ReturnType<typeof readPaymentBuilder>>;
export type BuilderAction = { action: "save"; draft: CheckoutDraft } | { action: "check" }
  | { action: "publish" } | { action: "disable"; method_id: string };
export type BuilderCommand = { operation_id: string; expected_revision: string; command: BuilderAction };

export async function changePaymentBuilder(csrf: string, command: BuilderCommand) {
  if (command.command.action === "publish" || command.command.action === "disable")
    return confirmPaymentAction(path, csrf, command);
  const result = await control(path, { method: "POST",
    headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(command) });
  if (result.applied !== true || result.operation_id !== command.operation_id) throw new Error("Invalid checkout acknowledgement");
}
