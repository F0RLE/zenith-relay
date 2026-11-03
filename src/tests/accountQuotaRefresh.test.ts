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

  test("Hosted cannot refresh local account credentials", async () => {
    const { calls, executor } = recordingExecutor();

    await expect(refreshOneAccountQuota("hosted", "account-local", executor)).rejects.toThrow("local pool");
    await expect(refreshAllAccountQuotas("hosted", executor)).rejects.toThrow("local pool");
    expect(calls).toEqual([]);
  });
});
