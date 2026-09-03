import { describe, expect, test } from "bun:test";
import {
  refreshAllAccountQuotas,
  refreshOneAccountQuota,
  type AccountQuotaRefreshExecutor,
} from "../src/features/relay/accountQuotaRefresh";

function recordingExecutor() {
  const calls: string[] = [];
  const executor: AccountQuotaRefreshExecutor = {
    refreshOneLocal: async (accountId) => { calls.push(`local:one:${accountId}`); },
    refreshAllLocal: async () => {
      calls.push("local:all");
      return [
        { status: "succeeded" as const },
        { status: "failed" as const },
        { status: "succeeded" as const },
      ];
    },
    refreshOneRemote: async (accountId) => { calls.push(`remote:one:${accountId}`); },
    refreshAllRemote: async () => {
      calls.push("remote:all");
      return { refreshed: 4, failed: 2 };
    },
  };
  return { calls, executor };
}

describe("account quota refresh", () => {
  test("keeps local account quota commands on the local pool", async () => {
    const { calls, executor } = recordingExecutor();

    await refreshOneAccountQuota("local", "account-local", executor);
    const report = await refreshAllAccountQuotas("local", executor);

    expect(calls).toEqual(["local:one:account-local", "local:all"]);
    expect(report).toEqual({ succeeded: 2, failed: 1 });
  });

  test("uses management actions and response counts outside the local pool", async () => {
    const { calls, executor } = recordingExecutor();

    await refreshOneAccountQuota("remote", "account-remote", executor);
    const report = await refreshAllAccountQuotas("zenith", executor);

    expect(calls).toEqual(["remote:one:account-remote", "remote:all"]);
    expect(report).toEqual({ succeeded: 4, failed: 2 });
  });
});
