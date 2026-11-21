import { control, record, uuidText } from "./control";

export interface OwnKey {
  id: string; name: string; prefix: string; status: "active" | "revoked" | "expired";
  operation_id: string; replaces_key_id: string | null; expires_at_s: number;
}
const uuid = (v: unknown): v is string => typeof v === "string" && uuidText.test(v);
function masked(v: unknown): OwnKey {
  if (!record(v) || !uuid(v.id) || !uuid(v.operation_id) || !("replaces_key_id" in v)
    || (v.replaces_key_id !== null && !uuid(v.replaces_key_id)) || typeof v.name !== "string"
    || !v.name.trim() || [...v.name].length > 80 || v.prefix !== "rlk_" + v.id.replaceAll("-", "").slice(0,12)
    || (v.status !== "active" && v.status !== "revoked" && v.status !== "expired")
    || typeof v.expires_at_s !== "number" || !Number.isSafeInteger(v.expires_at_s) || v.expires_at_s <= 0 || v.expires_at_s > 253402300799
    || "secret" in v || "verifier" in v) throw new Error("Invalid masked key");
  return { id:v.id, name:v.name, prefix:v.prefix as string, status:v.status, operation_id:v.operation_id,
    replaces_key_id:v.replaces_key_id, expires_at_s:v.expires_at_s };
}
export async function readKeys(filter?: { before: string } | { operation_id: string }, signal?: AbortSignal) {
  const data = await control("keys" + (filter ? "?" + new URLSearchParams(filter) : ""), signal ? { signal } : {});
  if (!Array.isArray(data.keys) || data.keys.length > 32 || (data.next_before !== null && !uuid(data.next_before)))
    throw new Error("Invalid key list");
  const keys = data.keys.map(masked);
  if (new Set(keys.map(k => k.id)).size !== keys.length
    || (data.next_before !== null && data.next_before !== keys.at(-1)?.id)
    || (filter && "operation_id" in filter && (keys.length > 1 || keys.some(k => k.operation_id !== filter.operation_id))))
    throw new Error("Invalid key page");
  return { keys, next: data.next_before };
}
export async function issueKey(csrf: string, op: string, name: string, original: string | null) {
  const v = await control("keys", { method: "POST", headers: { "Content-Type":"application/json", "X-Relay-CSRF":csrf },
    body: JSON.stringify({ operation_id:op, name, replaces_key_id:original }) });
  const key = masked(v.key);
  if (key.operation_id !== op || key.name !== name || key.replaces_key_id !== original) throw new Error("Unexpected key record");
  if (v.secret === undefined) return { key, secret:null };
  if (typeof v.secret !== "string" || !/^rlk_[a-f0-9]{32}\.[a-f0-9]{64}$/.test(v.secret)
    || !v.secret.startsWith(`rlk_${key.id.replaceAll("-", "")}.`)) throw new Error("Invalid new credential");
  return { key, secret:v.secret };
}
export async function revokeKey(csrf: string, id: string) {
  const v = await control("keys/" + encodeURIComponent(id), { method:"DELETE", headers:{"X-Relay-CSRF":csrf} });
  if (v.id !== id || v.revoked !== true) throw new Error("Revocation not confirmed");
}
