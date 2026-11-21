import type { PoolRoutingPolicy, RotationPreset } from "../../../../../desktop/frontend/src/features/relay/api/types";
import type { OwnerConnection } from "./model";

export { rotationModeName, usesRequestShares } from "../../../../../desktop/frontend/src/features/relay/pages/pool/rotationModes";
export type { PoolRoutingPolicy, RotationPreset };

// Hosted preview presets only; live routing remains server-owned.
export function applyRotationPreset(policy: PoolRoutingPolicy, preset: RotationPreset): PoolRoutingPolicy {
  const mode = preset === "recommended" ? "automatic" : preset === "even" ? "round_robin" : "failover";
  const defaults = policy.members.map(member => ({ ...member, weight: 1, maxConcurrency: 0 }));
  const members = preset === "accounts_first"
    ? [...defaults.filter(member => member.kind === "account"), ...defaults.filter(member => member.kind === "source")]
    : defaults;
  return { ...policy, mode, members };
}

export function currentRotationPreset(policy: PoolRoutingPolicy): RotationPreset | "custom" {
  if (policy.members.some(member => member.maxConcurrency !== 0)) return "custom";
  if (policy.mode === "automatic") return policy.members.every(member => member.weight === policy.members[0]?.weight) ? "recommended" : "custom";
  if (policy.mode === "round_robin") return "even";
  if (policy.mode === "failover") {
    let sourceSeen = false;
    for (const member of policy.members) {
      if (member.kind === "source") sourceSeen = true;
      else if (sourceSeen) return "custom";
    }
    return "accounts_first";
  }
  return "custom";
}

export function poolRotationKey(member: { kind: "account" | "source"; id: string }) {
  return `${member.kind}:${member.id}`;
}

/** Presentation-only policy. No admission, routing, or server persistence occurs here. */
export function reconcilePoolRotation(policy: PoolRoutingPolicy, connections: readonly OwnerConnection[]): PoolRoutingPolicy {
  const available = connections.filter(connection => connection.inPool && !connection.trashed).map(connection => ({
    kind: connection.kind === "api" ? "source" as const : "account" as const,
    id: connection.id,
  }));
  const keys = new Set(available.map(poolRotationKey));
  const members = policy.members.filter(member => keys.has(poolRotationKey(member)));
  const retained = new Set(members.map(poolRotationKey));
  for (const member of available) {
    if (!retained.has(poolRotationKey(member))) {
      members.push({ ...member, weight: 1, maxConcurrency: 0 });
      retained.add(poolRotationKey(member));
    }
  }
  if (members.length === policy.members.length && members.every((member, index) => member === policy.members[index])) return policy;
  return { ...policy, members };
}

export function movePoolRotationMember(policy: PoolRoutingPolicy, from: number, to: number): PoolRoutingPolicy {
  if (policy.mode === "automatic" || from === to || !policy.members[from] || !policy.members[to]) return policy;
  const members = [...policy.members];
  const [member] = members.splice(from, 1);
  members.splice(to, 0, member!);
  return { ...policy, members };
}

export function updatePoolRotationMember(policy: PoolRoutingPolicy, key: string, field: "weight" | "maxConcurrency", value: number): PoolRoutingPolicy {
  if (!Number.isFinite(value)) return policy;
  const bounded = Math.min(field === "weight" ? 100 : 1024, Math.max(field === "weight" ? 1 : 0, Math.trunc(value)));
  return { ...policy, members: policy.members.map(member => poolRotationKey(member) === key ? { ...member, [field]: bounded } : member) };
}
