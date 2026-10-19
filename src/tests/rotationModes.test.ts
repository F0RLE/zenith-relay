import { describe, expect, test } from "bun:test";
import type { PoolRoutingPolicy } from "../src/features/relay/api/types";
import { applyRotationPreset, currentRotationPreset, rotationModeName, supportsRotationEditing, usesRequestShares } from "../src/features/relay/pages/pool/rotationModes";
import { applyPoolRoutingEdits } from "../src/features/relay/pages/pool/poolRoutingEdits";

function policy(): PoolRoutingPolicy {
  return { version: 3, mode: "automatic", members: [
    { kind: "source", id: "api-a", weight: 17, maxConcurrency: 2 },
    { kind: "account", id: "account-a", weight: 83, maxConcurrency: 1 },
    { kind: "source", id: "api-b", weight: 25, maxConcurrency: 0 },
    { kind: "account", id: "account-b", weight: 75, maxConcurrency: 3 },
  ] };
}

describe("rotation modes and presets", () => {
  test("equal automatic shares are recommended regardless of their scale", () => {
    for (const weight of [1, 17, 100]) {
      const plain = { ...policy(), members: policy().members.map((member) => ({ ...member, weight, maxConcurrency: 0 })) };
      expect(currentRotationPreset(plain)).toBe("recommended");
    }
    expect(currentRotationPreset({ ...policy(), members: [] })).toBe("recommended");
  });

  test("presets restore controls while preserving inventory and relative order inside each group", () => {
    const saved = policy();
    for (const preset of ["recommended", "even", "accounts_first"] as const) {
      const applied = applyRotationPreset(saved, preset);
      expect(new Map(applied.members.map((member) => [member.id, member]))).toEqual(new Map(saved.members.map((member) => [member.id, { ...member, weight: 1, maxConcurrency: 0 }])));
      expect(applied.version).toBe(saved.version);
      expect(currentRotationPreset(applied)).toBe(preset);
      expect(applyRotationPreset(applied, preset)).toEqual(applied);
      if (preset !== "accounts_first") expect(applied.members.map((member) => member.id)).toEqual(saved.members.map((member) => member.id));
      expect(saved).toEqual(policy());
    }
    const reserve = applyRotationPreset(saved, "accounts_first");
    expect(reserve.mode).toBe("failover");
    expect(reserve.members.map((member) => member.id)).toEqual(["account-a", "account-b", "api-a", "api-b"]);
    expect(applyRotationPreset(reserve, "recommended").members).toEqual(reserve.members);
    expect(applyRotationPreset(reserve, "even").mode).toBe("round_robin");
  });

  test("presets rebase onto current membership without reviving removed members", () => {
    const saved = policy();
    saved.members = saved.members.filter((member) => member.id !== "account-a");
    saved.members.push({ kind: "account", id: "new", weight: 13, maxConcurrency: 7 });
    const edited = applyPoolRoutingEdits(saved, [{ type: "preset", preset: "accounts_first" }]);
    expect(edited.members.map((member) => member.id)).toEqual(["account-b", "new", "api-a", "api-b"]);
    expect(edited.members[1]).toMatchObject({ weight: 1, maxConcurrency: 0 });
  });

  test("selected presets are recognized when the pool has only one member kind or is empty", () => {
    const saved = policy();
    for (const members of [[], saved.members.filter((member) => member.kind === "account"), saved.members.filter((member) => member.kind === "source")]) {
      for (const preset of ["recommended", "even", "accounts_first"] as const) {
        expect(currentRotationPreset(applyRotationPreset({ ...saved, members }, preset))).toBe(preset);
      }
    }
  });

  test("equal and weighted cycling are distinct without discarding stored shares", () => {
    const saved = { ...policy(), mode: "round_robin" as const };
    expect(usesRequestShares(saved.mode)).toBe(false);
    const weighted = applyPoolRoutingEdits(saved, [{ type: "mode", mode: "weighted_round_robin" }]);
    expect(usesRequestShares(weighted.mode)).toBe(true);
    expect(weighted.members).toEqual(saved.members);
    expect(rotationModeName(weighted.mode)).toBe("roundRobin");
    expect(rotationModeName("failover")).toBe("failover");
    expect(rotationModeName("in_order")).toBe("roundRobin");
  });

  test("old servers retain their editor but cannot apply new presets", () => {
    const old = { ...policy(), version: 2 as const };
    expect(supportsRotationEditing(old, ["rotation_v2"])).toBe(true);
    expect(supportsRotationEditing({ ...old, mode: "failover" }, ["rotation_v2"])).toBe(false);
    expect(supportsRotationEditing({ ...old, mode: "weighted_round_robin" }, ["rotation_v2"])).toBe(false);
    expect(supportsRotationEditing(policy(), ["rotation_v2"])).toBe(false);
    expect(supportsRotationEditing(policy(), ["rotation_v3"])).toBe(true);
    expect(supportsRotationEditing(undefined, ["rotation_v3"])).toBe(false);
    expect(applyRotationPreset(old, "accounts_first")).toBe(old);
  });

  test("custom limits and shares are not disguised as a default preset", () => {
    expect(currentRotationPreset(policy())).toBe("custom");
    const plain = { ...policy(), members: policy().members.map((member) => ({ ...member, weight: 100, maxConcurrency: 0 })) };
    expect(currentRotationPreset(plain)).toBe("recommended");
    expect(currentRotationPreset(applyRotationPreset(plain, "even"))).toBe("even");
    expect(currentRotationPreset(applyRotationPreset(plain, "accounts_first"))).toBe("accounts_first");
  });
});
