import { describe, expect, test } from "bun:test";
import { updatePoolMembership, type PoolMembershipExecutor } from "../src/features/relay/poolMembership";

function recordingExecutor() {
  const calls: Array<{ target: "local" | "remote"; accountIds: string[]; sourceIds: string[]; inPool: boolean }> = [];
  const executor: PoolMembershipExecutor = {
    setLocal: async (accountIds, sourceIds, inPool) => { calls.push({ target: "local", accountIds, sourceIds, inPool }); },
    setRemote: async (accountIds, sourceIds, inPool) => { calls.push({ target: "remote", accountIds, sourceIds, inPool }); },
  };
  return { calls, executor };
}

describe("pool membership operation", () => {
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
