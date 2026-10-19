import type { PoolRoutingMode, PoolRoutingPolicy, PoolRoutingSnapshot } from "../../api/types";

export type RotationPreset = "recommended" | "even" | "accounts_first";

export function rotationModeName(mode: PoolRoutingMode) {
  return mode === "automatic" ? "automatic" : mode === "failover" ? "failover" : "roundRobin";
}

export function usesRequestShares(mode: PoolRoutingMode) {
  return mode === "automatic" || mode === "weighted_round_robin";
}

export function supportsRotationEditing(policy: PoolRoutingSnapshot | undefined, features: readonly string[]): policy is PoolRoutingPolicy {
  return policy != null && ((policy.version === 3 && features.includes("rotation_v3"))
    || (policy.version === 2 && features.includes("rotation_v2")
      && (policy.mode === "automatic" || policy.mode === "in_order" || policy.mode === "round_robin")));
}

export function applyRotationPreset(policy: PoolRoutingPolicy, preset: RotationPreset): PoolRoutingPolicy {
  if (policy.version !== 3) return policy;
  const mode = preset === "recommended" ? "automatic" : preset === "even" ? "round_robin" : "failover";
  const members = preset === "accounts_first"
    ? [...policy.members.filter((member) => member.kind === "account"), ...policy.members.filter((member) => member.kind === "source")]
    : policy.members;
  return { ...policy, mode, members };
}

export function currentRotationPreset(policy: PoolRoutingPolicy): RotationPreset | "custom" {
  if (policy.members.some((member) => member.maxConcurrency !== 0)) return "custom";
  if (policy.mode === "automatic") return policy.members.every((member) => member.weight === 100) ? "recommended" : "custom";
  if (policy.mode === "round_robin" || policy.mode === "in_order") return "even";
  if (policy.mode === "failover") {
    let sourceSeen = false;
    for (const member of policy.members) {
      if (member.kind === "source") sourceSeen = true;
      else if (sourceSeen) return "custom";
    }
    return policy.members.some((member) => member.kind === "account") && sourceSeen ? "accounts_first" : "custom";
  }
  return "custom";
}
