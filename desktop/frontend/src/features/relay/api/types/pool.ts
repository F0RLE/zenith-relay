export type PoolRoutingMode = "automatic" | "subscription_expiry" | "round_robin" | "weighted_round_robin" | "failover";
export type RotationPreset = "recommended" | "even" | "accounts_first";
export type PoolRotationView = { inventoryRevision?: number; preset: RotationPreset | null; memberOrder: string[] };
export type PoolRoutingEdit =
  | { type: "mode"; mode: PoolRoutingMode }
  | { type: "preset"; preset: RotationPreset }
  | { type: "move"; member: string; target: string; placement: "before" | "after" }
  | { type: "member"; member: string; field: "weight" | "maxConcurrency"; value: number };
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
