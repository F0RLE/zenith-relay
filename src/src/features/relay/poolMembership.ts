import { relayCommands } from "./api/commands";
import type { OAuthClientKind, RelayMode } from "./api/types";

export function canAccountJoinPool(account: {
  oauthClientKind?: OAuthClientKind;
  subscription?: { planType?: string | null };
  plan?: string | null;
}) {
  const plan = (account.subscription?.planType ?? account.plan)?.trim().toLowerCase().replace(/[^a-z0-9]/g, "");
  return account.oauthClientKind !== "excel_bps"
    || !["free", "freeplan", "freetier", "chatgptfree", "chatgptfreeplan", "chatgptfreetier"].includes(plan ?? "");
}

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
