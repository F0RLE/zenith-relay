/** in_order is retained only to preserve an imported cycling policy. */
export type PoolRoutingMode = "automatic" | "in_order" | "round_robin" | "weighted_round_robin" | "failover";
export type LegacyPoolRoutingMode = "smart" | "in_order" | "round_robin";
export type PoolRoutingMember = {
  kind: "account" | "source";
  id: string;
  weight: number;
  maxConcurrency: number;
};
export type PoolRoutingPolicy = {
  version: 2 | 3;
  mode: PoolRoutingMode;
  members: PoolRoutingMember[];
};
/** Older saved policy is read-only until the native migration succeeds. */
export type LegacyPoolRoutingPolicy = {
  version: 1;
  mode: LegacyPoolRoutingMode;
  members: PoolRoutingMember[];
};
export type PoolRoutingSnapshot = PoolRoutingPolicy | LegacyPoolRoutingPolicy;
