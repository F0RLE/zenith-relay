import { control, record, uuidText } from "./control";
import { confirmSupplyAction } from "./ownerSession";

export type PoolMode = "automatic" | "failover";
type RotationMember = { source_id: string; label: string; paused: boolean };
export type PoolRotation = {
  scope_id: string; revision: string; mode: PoolMode; members: RotationMember[];
};
export type PoolRotationChange = {
  operation_id: string; scope_id: string; expected_revision: string; mode: PoolMode; member_order: string[];
};

const path = "admin/pools/rotation";
const invalid = (): never => { throw new Error("Invalid pool rotation"); };
const uuid = (v: unknown): v is string => typeof v === "string" && v.length === 36
  && uuidText.test(v) && v !== "00000000-0000-0000-0000-000000000000";

export function isPoolMode(value: unknown): value is PoolMode {
  return value === "automatic" || value === "failover";
}

export function parsePoolRotation(data: Record<string, unknown>): PoolRotation {
  const { scope_id, revision, mode, members } = data;
  if (!uuid(scope_id) || typeof revision !== "string" || revision.length !== 64 || !/^[a-f0-9]+$/.test(revision)
    || !isPoolMode(mode) || !Array.isArray(members) || members.length > 128) return invalid();
  const seen = new Set<string>();
  return { scope_id, revision, mode, members: members.map((member: unknown) => {
    if (!record(member) || !uuid(member.source_id) || seen.has(member.source_id)
      || typeof member.label !== "string" || !member.label.trim() || member.label.length > 96 || /\p{Cc}/u.test(member.label)
      || typeof member.paused !== "boolean") return invalid();
    seen.add(member.source_id);
    return { source_id: member.source_id, label: member.label, paused: member.paused };
  }) };
}

export async function readPoolRotation(signal: AbortSignal) {
  return parsePoolRotation(await control(path, { signal }));
}

export function changePoolRotation(csrf: string, command: PoolRotationChange) {
  return confirmSupplyAction(path, csrf, command);
}
