import { describe, expect, test } from "bun:test";
import type { PoolRoutingPolicy } from "../src/features/relay/api/types";
import { rotationModeName, supportsRotationEditing, usesManualOrder, usesRequestShares } from "../src/features/relay/pages/pool/rotationModes";

function policy(): PoolRoutingPolicy {
  return { version: 3, mode: "automatic", members: [
    { kind: "source", id: "api-a", weight: 17, maxConcurrency: 2 },
    { kind: "account", id: "account-a", weight: 83, maxConcurrency: 1 },
    { kind: "source", id: "api-b", weight: 25, maxConcurrency: 0 },
    { kind: "account", id: "account-b", weight: 75, maxConcurrency: 3 },
  ] };
}

describe("rotation modes and presets", () => {
  test("mode labels and visible controls represent all backend strategies", () => {
    expect(usesRequestShares("automatic")).toBe(true);
    expect(usesRequestShares("subscription_expiry")).toBe(true);
    expect(usesManualOrder("subscription_expiry")).toBe(false);
    expect(usesManualOrder("automatic")).toBe(false);
    expect(usesManualOrder("round_robin")).toBe(true);
    expect(usesManualOrder("weighted_round_robin")).toBe(true);
    expect(usesManualOrder("failover")).toBe(true);
    expect(rotationModeName("subscription_expiry")).toBe("subscriptionExpiry");
    expect(usesRequestShares("round_robin")).toBe(false);
    expect(usesRequestShares("weighted_round_robin")).toBe(true);
    expect(usesRequestShares("failover")).toBe(false);
    expect(rotationModeName("weighted_round_robin")).toBe("roundRobin");
    expect(rotationModeName("failover")).toBe("failover");
  });

  test("editing requires the current native policy and capability", () => {
    for (const version of [1, 2, 4]) {
      const invalidSnapshot = { ...policy(), version } as unknown as PoolRoutingPolicy;
      expect(supportsRotationEditing(invalidSnapshot, ["rotation_v3"])).toBe(false);
    }
    expect(supportsRotationEditing(policy(), ["rotation_v2"])).toBe(false);
    expect(supportsRotationEditing(policy(), ["rotation_v3"])).toBe(false);
    expect(supportsRotationEditing(policy(), ["rotation_v3", "rotation_edits"])).toBe(true);
    expect(supportsRotationEditing(undefined, ["rotation_v3"])).toBe(false);
  });

});
