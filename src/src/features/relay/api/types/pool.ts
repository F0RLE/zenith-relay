export type PoolRoutingMode = "automatic" | "round_robin" | "weighted_round_robin" | "failover";
export type PoolRoutingMember = {
  kind: "account" | "source";
  id: string;
  weight: number;
  maxConcurrency: number;
};
export type PoolRoutingPolicy = {
  version: 3;
  mode: PoolRoutingMode;
  members: PoolRoutingMember[];
};
