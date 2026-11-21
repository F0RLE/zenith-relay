import { control, record, uuidText } from "./control";
import { isPoolMode, type PoolMode } from "./poolRotation";
import { validDeclaredModel } from "./sourceIntake";

interface Dispatches {
  source_id: string;
  source: string;
  model: string;
  requests: number;
}

export interface PoolActivity {
  strategy: PoolMode | null;
  member_order: string[];
  items: Dispatches[];
}

const invalid = (): never => { throw new Error("Invalid pool activity"); };

export function parsePoolActivity(value: unknown): PoolActivity {
  if (!record(value) || (!isPoolMode(value.strategy) && value.strategy !== null)
    || !Array.isArray(value.member_order) || value.member_order.length > 128
    || value.member_order.some((id: unknown) => typeof id !== "string" || id.length !== 36 || !uuidText.test(id))
    || new Set(value.member_order).size !== value.member_order.length || (value.strategy !== "failover" && value.member_order.length)
    || !Array.isArray(value.items) || value.items.length > 128 || (value.strategy === null && value.items.length))
    return invalid();
  const seen = new Set<string>();
  const items = value.items.map((row: unknown): Dispatches => {
    if (!record(row) || typeof row.source_id !== "string" || !uuidText.test(row.source_id)
      || typeof row.source !== "string" || !row.source.trim() || row.source.length > 96 || /\p{Cc}/u.test(row.source)
      || typeof row.model !== "string" || !validDeclaredModel(row.model)
      || typeof row.requests !== "number" || !Number.isSafeInteger(row.requests) || row.requests < 1 || row.requests > 128)
      return invalid();
    const key = `${row.source_id}:${row.model}`;
    if (seen.has(key)) return invalid();
    seen.add(key);
    return { source_id: row.source_id, source: row.source, model: row.model, requests: row.requests };
  });
  return { strategy: value.strategy, member_order: value.member_order, items };
}

export async function readPoolActivity(signal: AbortSignal) {
  return parsePoolActivity(await control("admin/pools/activity", { signal }));
}
