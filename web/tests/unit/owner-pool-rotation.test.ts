import { describe, expect, test } from "bun:test";
import type { OwnerConnection } from "../../src/features/owner/model";
import { applyRotationPreset, currentRotationPreset, movePoolRotationMember, poolRotationKey, reconcilePoolRotation, updatePoolRotationMember, usesRequestShares } from "../../src/features/owner/poolRotation";
import type { PoolRoutingPolicy } from "../../src/features/owner/poolRotation";

const connection = (id: string, kind: "account" | "api" = "account", changes: Partial<OwnerConnection> = {}): OwnerConnection => ({ id, kind, name: id, family: "Test", models: [], inPool: true, paused: false, trashed: false, ...changes });
const inventory = [connection("api", "api"), connection("account-a"), connection("account-b")];
const initial = (): PoolRoutingPolicy => reconcilePoolRotation({ version: 3, mode: "automatic", members: [] }, inventory);

describe("owner pool rotation preview", () => {
  test("uses current local policy shape and only admitted, non-trashed inventory", () => {
    const policy = reconcilePoolRotation(initial(), [...inventory, connection("outside", "api", { inPool: false }), connection("trash", "account", { trashed: true })]);
    expect(policy).toEqual(initial());
    expect(policy.members.map(poolRotationKey)).toEqual(["source:api", "account:account-a", "account:account-b"]);
    expect(policy.members.every(member => member.weight === 1 && member.maxConcurrency === 0)).toBe(true);
    expect(reconcilePoolRotation(policy, inventory)).toBe(policy);
  });

  test("retains edits and order, prunes removals, and adds new members without reviving old settings", () => {
    let policy = updatePoolRotationMember({ ...initial(), mode: "round_robin" }, "account:account-b", "weight", 12);
    policy = movePoolRotationMember(policy, 2, 0);
    const next = reconcilePoolRotation(policy, [...inventory.slice(1), connection("new")]);
    expect(next.members.map(member => member.id)).toEqual(["account-b", "account-a", "new"]);
    expect(next.members[0]!.weight).toBe(12);
    const removed = reconcilePoolRotation(next, [inventory[1]!]);
    const restored = reconcilePoolRotation(removed, inventory);
    expect(restored.members.find(member => member.id === "account-b")!.weight).toBe(1);
    expect(policy.members[0]!.weight).toBe(12);
  });

  test("qualifies identity by connection kind and preserves paused members", () => {
    const policy = reconcilePoolRotation(initial(), [connection("same"), connection("same", "api", { paused: true })]);
    expect(policy.members.map(poolRotationKey)).toEqual(["account:same", "source:same"]);
    const changed = updatePoolRotationMember(policy, "source:same", "weight", 7);
    expect(changed.members.map(member => member.weight)).toEqual([1, 7]);
  });

  test("reuses Desktop presets and makes weighted cycling explicit", () => {
    const original = updatePoolRotationMember(initial(), "source:api", "maxConcurrency", 8);
    expect(currentRotationPreset(original)).toBe("custom");
    const reserve = applyRotationPreset(original, "accounts_first");
    expect(reserve.mode).toBe("failover");
    expect(reserve.members.map(member => member.kind)).toEqual(["account", "account", "source"]);
    expect(reserve.members.every(member => member.weight === 1 && member.maxConcurrency === 0)).toBe(true);
    for (const preset of ["recommended", "even", "accounts_first"] as const) expect(currentRotationPreset(applyRotationPreset(original, preset))).toBe(preset);
    expect(usesRequestShares("round_robin")).toBe(false);
    expect(usesRequestShares("weighted_round_robin")).toBe(true);
    expect(original.members[0]!.maxConcurrency).toBe(8);
  });

  test("moves only existing members in manual-order strategies", () => {
    const auto = initial();
    expect(movePoolRotationMember(auto, 0, 2)).toBe(auto);
    const manual = { ...auto, mode: "round_robin" as const };
    expect(movePoolRotationMember(manual, -1, 0)).toBe(manual);
    expect(movePoolRotationMember(manual, 0, 9)).toBe(manual);
    expect(movePoolRotationMember(manual, 0, 0)).toBe(manual);
    expect(movePoolRotationMember(manual, 0, 2).members.map(member => member.id)).toEqual(["account-a", "account-b", "api"]);
    expect(manual.members.map(member => member.id)).toEqual(["api", "account-a", "account-b"]);
  });

  test("bounds controls without accepting nonfinite numbers or modifying other members", () => {
    const policy = initial();
    expect(updatePoolRotationMember(policy, "source:api", "weight", NaN)).toBe(policy);
    expect(updatePoolRotationMember(policy, "source:api", "weight", Infinity)).toBe(policy);
    expect(updatePoolRotationMember(policy, "source:api", "weight", 999).members[0]!.weight).toBe(100);
    expect(updatePoolRotationMember(policy, "source:api", "weight", -1).members[0]!.weight).toBe(1);
    expect(updatePoolRotationMember(policy, "source:api", "maxConcurrency", 9999).members[0]!.maxConcurrency).toBe(1024);
    expect(updatePoolRotationMember(policy, "source:api", "maxConcurrency", -3).members[0]!.maxConcurrency).toBe(0);
    expect(updatePoolRotationMember(policy, "source:api", "maxConcurrency", 3.8).members[0]!.maxConcurrency).toBe(3);
    expect(policy.members[0]!.maxConcurrency).toBe(0);
  });
});
