import { control, record, uuidText } from "./control";

export interface SavedSource {
  id: string;
  label: string | null;
  credential_state: "stored" | "revoked";
  proxy_bound: boolean;
  verified_models: string[];
}
export interface ConnectionPage { items: SavedSource[]; next_after: string | null }
const invalid = (): never => { throw new Error("Invalid connection inventory"); };
const uuid = (v: unknown): v is string => typeof v === "string" && v.length === 36 && uuidText.test(v);
const model = (v: unknown): v is string => typeof v === "string" && /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(v) && v.trim() === v;

export function parseConnections(raw: unknown, after?: string): ConnectionPage {
  if (!record(raw) || !Array.isArray(raw.items) || raw.items.length > 32) return invalid();
  let previous = after ?? "";
  const items = raw.items.map((v: unknown): SavedSource => {
    if (!record(v) || !uuid(v.id) || v.id <= previous
      || (v.label !== null && (typeof v.label !== "string" || !v.label || v.label.trim() !== v.label
        || v.label.length > 96 || /\p{Cc}/u.test(v.label)))
      || (v.credential_state !== "stored" && v.credential_state !== "revoked") || typeof v.proxy_bound !== "boolean"
      || !Array.isArray(v.verified_models) || v.verified_models.length > 128 || !v.verified_models.every(model)
      || v.verified_models.some((m, i, all) => i > 0 && m <= all[i - 1]!)
      || (v.credential_state === "revoked" && v.verified_models.length !== 0)) return invalid();
    previous = v.id;
    return { id: v.id, label: v.label, credential_state: v.credential_state, proxy_bound: v.proxy_bound, verified_models: v.verified_models };
  });
  const next = raw.next_after;
  if (next !== null && (!uuid(next) || items.length !== 32 || next !== previous)) return invalid();
  return { items, next_after: next };
}

export const readConnections = async (after?: string) =>
  parseConnections(await control("admin/connections" + (after ? "?" + new URLSearchParams({after}) : "")), after);
