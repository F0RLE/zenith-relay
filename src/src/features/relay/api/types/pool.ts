/** Stored values; the editor presents both manual values as the Manual mode. */
export type PoolRoutingMode = "automatic" | "in_order" | "round_robin";
export type LegacyPoolRoutingMode = "smart" | "in_order" | "round_robin";
export type PoolRoutingMember = {
  kind: "account" | "source";
  id: string;
  weight: number;
  maxConcurrency: number;
};
export type PoolRoutingPolicy = {
  version: 2;
  mode: PoolRoutingMode;
  members: PoolRoutingMember[];
};
/** Snapshot shape accepted from an older server during compatibility reads. */
export type LegacyPoolRoutingPolicy = {
  version: 1;
  mode: LegacyPoolRoutingMode;
  members: PoolRoutingMember[];
};
export type PoolRoutingSnapshot = PoolRoutingPolicy | LegacyPoolRoutingPolicy;
