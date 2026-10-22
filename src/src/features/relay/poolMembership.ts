import { relayCommands } from "./api/commands";
import type { OAuthClientKind, RelayMode } from "./api/types";

export function canAccountJoinPool(account: {
  oauthClientKind?: OAuthClientKind;
  subscription?: { planType?: string | null };
  plan?: string | null;
  recoveryRequired?: boolean;
  lastErrorCode?: string | null;
}) {
  if (account.oauthClientKind !== "excel_bps" && account.oauthClientKind !== "siwc") return false;
  if (account.recoveryRequired) return false;
  if (account.lastErrorCode === "siwc_plan_use_disabled") return false;
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
};

const relayPoolMembershipExecutor: PoolMembershipExecutor = {
  setLocal: relayCommands.setPoolMembership,
};

/** Applies pool membership locally only for the personal local-pool mode. */
export function updatePoolMembership(
  mode: RelayMode,
  input: PoolMembershipInput,
  executor: PoolMembershipExecutor = relayPoolMembershipExecutor,
) {
  const accountIds = [...input.accountIds];
  const sourceIds = [...input.sourceIds];
  if (mode !== "local") return Promise.reject(new Error("Pool membership requires the local pool"));
  return executor.setLocal(accountIds, sourceIds, input.inPool);
}
