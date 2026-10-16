import { describe, expect, test } from "bun:test";
import { canAccountJoinPool, updatePoolMembership, type PoolMembershipExecutor } from "../src/features/relay/poolMembership";

function recordingExecutor() {
  const calls: Array<{ target: "local" | "remote"; accountIds: string[]; sourceIds: string[]; inPool: boolean }> = [];
  const executor: PoolMembershipExecutor = {
    setLocal: async (accountIds, sourceIds, inPool) => { calls.push({ target: "local", accountIds, sourceIds, inPool }); },
    setRemote: async (accountIds, sourceIds, inPool) => { calls.push({ target: "remote", accountIds, sourceIds, inPool }); },
  };
  return { calls, executor };
}

describe("pool membership operation", () => {
  test("allows native accounts including Free and unknown plans", () => {
    for (const planType of ["free", "chatgpt_free_tier", "plus", null]) {
      expect(canAccountJoinPool({ oauthClientKind: "codex", subscription: { planType } })).toBe(true);
      expect(canAccountJoinPool({ subscription: { planType } })).toBe(true);
    }
  });

  test("rejects only known Free BPS plans in account and import previews", () => {
    for (const plan of ["free", "Free plan", " chatgpt_free_tier "]) {
      expect(canAccountJoinPool({ oauthClientKind: "excel_bps", subscription: { planType: plan } })).toBe(false);
      expect(canAccountJoinPool({ oauthClientKind: "excel_bps", plan })).toBe(false);
    }
    for (const plan of ["plus", "pro", "business", "enterprise", "unknown", null]) {
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

  test("uses the managed membership action outside local mode", async () => {
    const { calls, executor } = recordingExecutor();
    await updatePoolMembership("remote", { accountIds: [], sourceIds: ["source-2"], inPool: false }, executor);
    await updatePoolMembership("zenith", { accountIds: ["account-2"], sourceIds: [], inPool: true }, executor);

    expect(calls).toEqual([
      { target: "remote", accountIds: [], sourceIds: ["source-2"], inPool: false },
      { target: "remote", accountIds: ["account-2"], sourceIds: [], inPool: true },
    ]);
  });
});
