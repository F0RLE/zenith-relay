import { relayCommands } from "./api/commands";
import type { DefaultServiceTier, PoolRoutingPolicy, RelayMode, RoutingStrategy } from "./api/types";

export type RoutingPolicy = {
  poolRouting?: PoolRoutingPolicy;
  expectedPoolRouting?: PoolRoutingPolicy;
  maxRetryCandidates: number;
  cooldownAfterFailures: number;
  keepLastCandidateAvailable: boolean;
  routingStrategy: RoutingStrategy;
  defaultServiceTier: DefaultServiceTier;
  subscriptionPlanOrder: string[];
};

export function persistRoutingPolicy(mode: RelayMode, policy: RoutingPolicy) {
  return mode === "local"
    ? relayCommands.updateRouting(policy.routingStrategy, policy.maxRetryCandidates, policy.cooldownAfterFailures, policy.keepLastCandidateAvailable, policy.defaultServiceTier, policy.subscriptionPlanOrder, policy.poolRouting, policy.expectedPoolRouting)
    : relayCommands.remoteAction({ type: "set_routing_policy" }, policy)
      .then(() => relayCommands.syncCodexDefaultServiceTier(policy.defaultServiceTier));
}
