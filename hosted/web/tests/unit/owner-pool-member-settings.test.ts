import { describe, expect, test } from "bun:test";
import { ownerMemberModelPolicy, ownerMemberModels, parseMemberPurchaseCost } from "../../src/features/owner/poolMemberSettings";
import { updateConnections } from "../../src/features/owner/model";
import type { OwnerConnection } from "../../src/features/owner/model";
import { previewOwnerAccount } from "../../src/preview/ownerAccounts";

const member = (): OwnerConnection => ({ id: "member", name: "Test account", family: "test", kind: "account", models: ["alpha", "beta"], inPool: true, paused: false, trashed: false });
const settings = { allowedModels: [], excludedModels: ["beta"], paused: true, purchaseCostMicroUsd: 12_500_001 };

describe("account pool editor preview", () => {
  test("model exclusions reuse Desktop rules without deleting discovered models", () => {
    const original = member();
    const saved = updateConnections([original], { type: "saveMember", id: original.id, settings })[0]!;
    expect(saved.models).toBe(original.models);
    expect(saved.paused).toBe(true);
    expect(ownerMemberModels(saved)).toEqual({ modelIds: ["alpha", "beta"], enabledModels: ["alpha"] });
    expect(original).toEqual(member());
    expect(ownerMemberModels({ ...saved, models: [...saved.models, "new-model"] }).enabledModels).toEqual(["alpha", "new-model"]);
  });

  test("selection is case insensitive, deduplicated and scoped to actual inventory", () => {
    const original = { ...member(), models: ["Alpha", "alpha", "Beta"], modelPolicy: { allowedModels: [], excludedModels: ["BETA", "missing"] } };
    expect(ownerMemberModels(original)).toEqual({ modelIds: ["Alpha", "Beta"], enabledModels: ["Alpha"] });
    expect(ownerMemberModelPolicy(original, ["Alpha", "Beta"])).toEqual({ allowedModels: [], excludedModels: ["missing"] });
    expect(ownerMemberModels({ ...member(), models: [] })).toEqual({ modelIds: [], enabledModels: [] });
  });

  test("saved switches replace legacy wildcard policy, so re-enabling works", () => {
    const original = { ...member(), modelPolicy: { allowedModels: [], excludedModels: ["a*"] } };
    expect(ownerMemberModels(original).enabledModels).toEqual(["beta"]);
    expect(ownerMemberModelPolicy(original, ["alpha", "beta"])).toEqual({ allowedModels: [], excludedModels: [] });
    const none = ownerMemberModelPolicy(original, []);
    expect(none.excludedModels).toEqual(["alpha", "beta"]);
    expect(ownerMemberModels({ ...original, modelPolicy: none }).enabledModels).toEqual([]);
  });

  test("purchase cost is exact microUSD and preserves unknown versus observed zero", () => {
    expect(parseMemberPurchaseCost("")).toBeNull();
    expect(parseMemberPurchaseCost("  ")).toBeNull();
    expect(parseMemberPurchaseCost("0")).toBe(0);
    expect(parseMemberPurchaseCost("12,500001")).toBe(12_500_001);
    expect(parseMemberPurchaseCost("0.000001")).toBe(1);
    expect(parseMemberPurchaseCost("1000000.000000")).toBe(1_000_000_000_000);
    for (const invalid of ["-1", "NaN", "Infinity", "1e2", "1.0000001", "1000000.000001", "1,2,3", "1_000", "999999999999999999999"]) {
      expect(parseMemberPurchaseCost(invalid)).toBeUndefined();
    }
  });

  test("saving cost retains all quota/economics evidence and unrelated accounts", () => {
    const original = { ...member(), account: previewOwnerAccount(Date.parse("2026-11-02T12:00:00Z"), "normal") };
    const other = { ...member(), id: "other" };
    const saved = updateConnections([original, other], { type: "saveMember", id: original.id, settings });
    expect(saved[0]!.account).toEqual({ ...original.account, purchaseCostMicroUsd: settings.purchaseCostMicroUsd });
    expect(saved[0]!.account!.quota).toBe(original.account.quota);
    expect(saved[1]).toBe(other);
    expect(saved[0]!.modelPolicy!.excludedModels).not.toBe(settings.excludedModels);
  });

  test("unknown presentation remains unknown when adding purchase cost", () => {
    const saved = updateConnections([member()], { type: "saveMember", id: "member", settings })[0]!;
    expect(saved.account).toMatchObject({ quotaStatus: "pending", apiEquivalent: null, quotaWindowUsage: null, purchaseCostMicroUsd: 12_500_001 });
    expect(updateConnections([saved], { type: "saveMember", id: "member", settings: { ...settings, purchaseCostMicroUsd: null } })[0]!.account!.purchaseCostMicroUsd).toBeNull();
  });

  test("missing, removed, trashed and API members cannot receive account settings", () => {
    for (const original of [{ ...member(), inPool: false }, { ...member(), trashed: true }, { ...member(), kind: "api" as const }]) {
      const current = [original];
      expect(updateConnections(current, { type: "saveMember", id: "member", settings })).toBe(current);
    }
    const current = [member()];
    expect(updateConnections(current, { type: "saveMember", id: "missing", settings })).toBe(current);
    for (const cost of [-1, NaN, Infinity, 0.5, 1_000_000_000_001]) {
      expect(updateConnections(current, { type: "saveMember", id: "member", settings: { ...settings, purchaseCostMicroUsd: cost } })).toBe(current);
    }
  });
});
