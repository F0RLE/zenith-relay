import { relayCommands } from "./api/commands";
import type { DefaultServiceTier, RelayMode, RoutingStrategy } from "./api/types";

export type RoutingPolicy = {
  maxRetryCandidates: number;
  cooldownAfterFailures: number;
  keepLastCandidateAvailable: boolean;
  routingStrategy: RoutingStrategy;
  defaultServiceTier: DefaultServiceTier;
  subscriptionPlanOrder: string[];
};

export function persistRoutingPolicy(mode: RelayMode, policy: RoutingPolicy) {
  return mode === "local"
    ? relayCommands.updateRouting(policy.routingStrategy, policy.maxRetryCandidates, policy.cooldownAfterFailures, policy.keepLastCandidateAvailable, policy.defaultServiceTier, policy.subscriptionPlanOrder)
    : relayCommands.remoteAction({ type: "set_routing_policy" }, policy)
      .then(() => relayCommands.syncCodexDefaultServiceTier(policy.defaultServiceTier));
}
