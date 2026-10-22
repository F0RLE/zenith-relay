import { relayCommands } from "./api/commands";
import type { RelayMode } from "./api/types";

export type AccountQuotaRefreshReport = {
  succeeded: number;
  failed: number;
};

type LocalQuotaRefreshResult = {
  status: "succeeded" | "failed";
};

export type AccountQuotaRefreshExecutor = {
  refreshOneLocal: (accountId: string) => Promise<unknown>;
  refreshAllLocal: () => Promise<readonly LocalQuotaRefreshResult[]>;
};

const relayAccountQuotaRefreshExecutor: AccountQuotaRefreshExecutor = {
  refreshOneLocal: relayCommands.refreshAccountQuota,
  refreshAllLocal: relayCommands.refreshAllAccountQuotas,
};

/** Refreshes one account through the runtime that owns its credential. */
export function refreshOneAccountQuota(
  mode: RelayMode,
  accountId: string,
  executor: AccountQuotaRefreshExecutor = relayAccountQuotaRefreshExecutor,
) {
  if (mode !== "local") return Promise.reject(new Error("Account quotas require the local pool"));
  return executor.refreshOneLocal(accountId);
}

/** Refreshes local account quotas and normalizes the report. */
export async function refreshAllAccountQuotas(
  mode: RelayMode,
  executor: AccountQuotaRefreshExecutor = relayAccountQuotaRefreshExecutor,
): Promise<AccountQuotaRefreshReport> {
  if (mode !== "local") throw new Error("Account quotas require the local pool");
  const results = await executor.refreshAllLocal();
  return {
    succeeded: results.filter((result) => result.status === "succeeded").length,
    failed: results.filter((result) => result.status === "failed").length,
  };
}
