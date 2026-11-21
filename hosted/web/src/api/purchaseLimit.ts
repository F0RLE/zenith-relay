import { control, csrfText, record, uuidText } from "./control";
import { confirmSupplyAction } from "./ownerSession";

const maxMicros = 9_223_372_036_854_775_807n;
const scale = 1_000_000n;

export function purchaseLimitMicros(dollars: string): string | undefined {
  const input = dollars.trim();
  if (!/^\d{1,13}(?:[.,]\d{1,6})?$/.test(input)) return;
  const [whole, fraction = ""] = input.split(/[.,]/);
  const amount = BigInt(whole!) * scale + BigInt(fraction.padEnd(6, "0"));
  if (amount <= maxMicros) return amount.toString();
}

export function purchaseLimitInput(micros: string | null) {
  if (micros === null) return "";
  const n = BigInt(micros);
  const fraction = (n % scale).toString().padStart(6, "0").replace(/0+$/, "");
  return `${n / scale}${fraction ? "." + fraction : ""}`;
}

type PurchaseLimit = {
  revision: string;
  month_utc: string;
  limit_micro_usd: string | null;
  accounted: bigint;
  outstanding: bigint;
  remaining: bigint | null;
  unpriced: boolean;
};

export type PurchaseLimitChange = {
  operation_id: string;
  expected_revision: string;
  limit_micro_usd: string | null;
};

const integer = (s: unknown): s is string => typeof s === "string" && /^(?:0|[1-9]\d{0,63})$/.test(s) && s.trim() === s;
const i64 = (s: unknown): s is string => integer(s) && BigInt(s) <= maxMicros;
const pathFor = (source: string) => {
  if (source.length !== 36 || !uuidText.test(source)) throw new Error("Invalid source ID");
  return `admin/connections/${source}/purchase-limit`;
};

export function parsePurchaseLimit(v: unknown, source: string): PurchaseLimit {
  if (!record(v) || v.source_id !== source || !i64(v.revision)
    || typeof v.month_utc !== "string" || !/^\d{4}-(?:0[1-9]|1[0-2])-01$/.test(v.month_utc) || v.month_utc.length !== 10
    || !(v.limit_micro_usd === null || i64(v.limit_micro_usd)) || typeof v.unpriced !== "boolean"
    || !integer(v.accounted_micro_usd) || !integer(v.outstanding_micro_usd)
    || (v.unpriced || v.limit_micro_usd === null ? v.remaining_micro_usd !== null : !i64(v.remaining_micro_usd))) {
    throw new Error("Invalid purchase limit");
  }
  return { revision: v.revision, month_utc: v.month_utc, limit_micro_usd: v.limit_micro_usd,
    accounted: BigInt(v.accounted_micro_usd), outstanding: BigInt(v.outstanding_micro_usd), unpriced: v.unpriced,
    remaining: typeof v.remaining_micro_usd === "string" ? BigInt(v.remaining_micro_usd) : null };
}

export const readPurchaseLimit = async (source: string) => parsePurchaseLimit(await control(pathFor(source)), source);

export async function changePurchaseLimit(source: string, csrf: string, change: PurchaseLimitChange) {
  if (csrf.length !== 64 || !csrfText.test(csrf)) throw new Error("Invalid CSRF token");
  return confirmSupplyAction(pathFor(source), csrf, change);
}
