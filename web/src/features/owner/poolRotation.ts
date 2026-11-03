import type { PoolRoutingPolicy } from "../../../../src/src/features/relay/api/types";
import type { OwnerConnection } from "./model";

export { applyRotationPreset, currentRotationPreset, rotationModeName, usesRequestShares } from "../../../../src/src/features/relay/pages/pool/rotationModes";
export type { RotationPreset } from "../../../../src/src/features/relay/pages/pool/rotationModes";
export type { PoolRoutingPolicy };

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
