import { relayCommands } from "./api/commands";
import type { RelayMode } from "./api/types";

export type PoolMembershipInput = {
  accountIds: readonly string[];
  sourceIds: readonly string[];
  inPool: boolean;
};

export type PoolMembershipExecutor = {
  setLocal: (accountIds: string[], sourceIds: string[], inPool: boolean) => Promise<unknown>;
  setRemote: (accountIds: string[], sourceIds: string[], inPool: boolean) => Promise<unknown>;
};

const relayPoolMembershipExecutor: PoolMembershipExecutor = {
  setLocal: relayCommands.setPoolMembership,
  setRemote: (accountIds, sourceIds, inPool) => relayCommands.remoteAction(
    { type: "set_pool_membership" },
    { accountIds, sourceIds, inPool },
  ),
};

/** Applies pool membership locally only for the personal local-pool mode. */
export function updatePoolMembership(
  mode: RelayMode,
  input: PoolMembershipInput,
  executor: PoolMembershipExecutor = relayPoolMembershipExecutor,
) {
  const accountIds = [...input.accountIds];
  const sourceIds = [...input.sourceIds];
  return mode === "local"
    ? executor.setLocal(accountIds, sourceIds, input.inPool)
    : executor.setRemote(accountIds, sourceIds, input.inPool);
}
