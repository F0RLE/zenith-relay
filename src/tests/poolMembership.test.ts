import { describe, expect, test } from "bun:test";
import { canAccountJoinPool, updatePoolMembership, type PoolMembershipExecutor } from "../src/features/relay/poolMembership";

test("SIWC without plan consent cannot join the pool", () => {
  expect(canAccountJoinPool({ oauthClientKind: "siwc", lastErrorCode: "siwc_plan_use_disabled" })).toBe(false);
  expect(canAccountJoinPool({ oauthClientKind: "siwc" })).toBe(true);
});

function recordingExecutor() {
  const calls: Array<{ target: "local"; accountIds: string[]; sourceIds: string[]; inPool: boolean }> = [];
  const executor: PoolMembershipExecutor = {
    setLocal: async (accountIds, sourceIds, inPool) => { calls.push({ target: "local", accountIds, sourceIds, inPool }); },
  };
  return { calls, executor };
}

describe("pool membership operation", () => {
  test("accounts awaiting local recovery cannot join the pool", () => {
    expect(canAccountJoinPool({ oauthClientKind: "codex", recoveryRequired: true })).toBe(false);
    expect(canAccountJoinPool({ oauthClientKind: "excel_bps", plan: "plus", recoveryRequired: true })).toBe(false);
  });
  test("offers explicit image-only membership for known Codex connections, not unknown legacy clients", () => {
    for (const planType of ["free", "chatgpt_free_tier", "plus", null]) {
      expect(canAccountJoinPool({ oauthClientKind: "codex", subscription: { planType } })).toBe(true);
      expect(canAccountJoinPool({ subscription: { planType } })).toBe(false);
    }
  });

  test("allows BPS membership regardless of the subscription plan", () => {
    for (const plan of ["free", "Free plan", " chatgpt_free_tier ", "plus", "pro", "business", "enterprise", "unknown", null]) {
      expect(canAccountJoinPool({ oauthClientKind: "excel_bps", subscription: { planType: plan } })).toBe(true);
      expect(canAccountJoinPool({ oauthClientKind: "excel_bps", plan })).toBe(true);
    }
  });

  test("uses local membership storage only in local mode", async () => {
    const { calls, executor } = recordingExecutor();
    await updatePoolMembership("local", { accountIds: ["account-1"], sourceIds: ["source-1"], inPool: true }, executor);

    expect(calls).toEqual([
      { target: "local", accountIds: ["account-1"], sourceIds: ["source-1"], inPool: true },
    ]);
  });

  test("Hosted cannot change local membership", async () => {
    const { calls, executor } = recordingExecutor();
    await expect(updatePoolMembership("hosted", { accountIds: [], sourceIds: ["source-2"], inPool: false }, executor)).rejects.toThrow("local pool");
    expect(calls).toEqual([]);
  });
});
