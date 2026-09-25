import { relayCommands } from "./api/commands";
import type { DefaultServiceTier, PoolRoutingPolicy, PoolRoutingSnapshot, RelayMode } from "./api/types";

export type RoutingPolicy = {
  basisPointsEnabled?: boolean;
  poolRouting?: PoolRoutingPolicy;
  expectedPoolRouting?: PoolRoutingSnapshot;
  maxRetryCandidates: number;
  defaultServiceTier: DefaultServiceTier;
};

export function persistRoutingPolicy(mode: RelayMode, policy: RoutingPolicy) {
  return mode === "local"
    ? relayCommands.updateRouting(policy.maxRetryCandidates, policy.defaultServiceTier, policy.poolRouting, policy.expectedPoolRouting, policy.basisPointsEnabled)
    : relayCommands.remoteAction({ type: "set_routing_policy" }, policy)
      .then(() => relayCommands.syncCodexDefaultServiceTier(policy.defaultServiceTier));
}
