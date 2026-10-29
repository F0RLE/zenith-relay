import { relayCommands } from "./api/commands";
import type { DefaultServiceTier, PoolRoutingPolicy, PoolRoutingSnapshot, RelayMode } from "./api/types";

export type RoutingPolicy = {
  poolRouting?: PoolRoutingPolicy;
  expectedPoolRouting?: PoolRoutingSnapshot;
  maxRetryCandidates: number;
  defaultServiceTier: DefaultServiceTier;
};

export function persistRoutingPolicy(mode: RelayMode, policy: RoutingPolicy) {
  if (mode !== "local") return Promise.reject(new Error("Routing requires the local pool"));
  return relayCommands.updateRouting(policy.maxRetryCandidates, policy.defaultServiceTier, policy.poolRouting, policy.expectedPoolRouting);
}
