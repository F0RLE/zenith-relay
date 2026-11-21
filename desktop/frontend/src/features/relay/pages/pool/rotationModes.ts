import type { PoolRoutingMode, PoolRoutingPolicy } from "../../api/types";

export function rotationModeName(mode: PoolRoutingMode) {
  return mode === "automatic" ? "automatic" : mode === "subscription_expiry" ? "subscriptionExpiry" : mode === "failover" ? "failover" : "roundRobin";
}

export function usesManualOrder(mode: PoolRoutingMode) {
  return mode === "round_robin" || mode === "weighted_round_robin" || mode === "failover";
}

export function usesRequestShares(mode: PoolRoutingMode) {
  return !usesManualOrder(mode) || mode === "weighted_round_robin";
}

export function supportsRotationEditing(policy: PoolRoutingPolicy | undefined, features: readonly string[]): policy is PoolRoutingPolicy {
  return policy != null && policy.version === 3 && features.includes("rotation_v3") && features.includes("rotation_edits");
}
