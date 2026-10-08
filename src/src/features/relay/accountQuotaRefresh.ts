import { relayCommands } from "./api/commands";
import type { RelayMode } from "./api/types";

export type AccountQuotaRefreshReport = {
  succeeded: number;
  failed: number;
};

type LocalQuotaRefreshResult = {
  status: "succeeded" | "failed";
};

type RemoteQuotaRefreshResult = {
  refreshed?: number;
  failed?: number;
};

export type AccountQuotaRefreshExecutor = {
  refreshOneLocal: (accountId: string) => Promise<unknown>;
  refreshAllLocal: () => Promise<readonly LocalQuotaRefreshResult[]>;
  refreshOneRemote: (accountId: string) => Promise<unknown>;
  refreshAllRemote: () => Promise<RemoteQuotaRefreshResult>;
};

const relayAccountQuotaRefreshExecutor: AccountQuotaRefreshExecutor = {
  refreshOneLocal: relayCommands.refreshAccountQuota,
  refreshAllLocal: relayCommands.refreshAllAccountQuotas,
  refreshOneRemote: (accountId) => relayCommands.remoteAction({ type: "refresh_account", id: accountId }),
  refreshAllRemote: () => relayCommands.remoteAction({ type: "refresh_all_quotas" }) as Promise<RemoteQuotaRefreshResult>,
};

/** Refreshes one account through the runtime that owns its credential. */
export function refreshOneAccountQuota(
  mode: RelayMode,
  accountId: string,
  executor: AccountQuotaRefreshExecutor = relayAccountQuotaRefreshExecutor,
) {
  return mode === "local"
    ? executor.refreshOneLocal(accountId)
    : executor.refreshOneRemote(accountId);
}

/** Refreshes all local or managed account quotas and normalizes the report. */
export async function refreshAllAccountQuotas(
  mode: RelayMode,
  executor: AccountQuotaRefreshExecutor = relayAccountQuotaRefreshExecutor,
): Promise<AccountQuotaRefreshReport> {
  if (mode === "local") {
    const localRefreshResults = await executor.refreshAllLocal();
    return {
      succeeded: localRefreshResults.filter((refreshResult) => refreshResult.status === "succeeded").length,
      failed: localRefreshResults.filter((refreshResult) => refreshResult.status === "failed").length,
    };
  }

  const remoteRefreshResult = await executor.refreshAllRemote();
  return { succeeded: remoteRefreshResult.refreshed ?? 0, failed: remoteRefreshResult.failed ?? 0 };
}
