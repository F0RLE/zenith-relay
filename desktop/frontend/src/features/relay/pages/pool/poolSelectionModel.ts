import type { AccountSummary } from "../../api/types";
import { accountMetadataSources } from "../../accountRoutes";
import { accountQuotaRefreshState } from "../../accountStatus";
import type { PoolMember } from "../../poolHelpers";

export const poolMemberKey = (member: PoolMember) => `${member.kind}:${member.id}`;

export function poolRefreshTargets(members: readonly PoolMember[], accounts: readonly AccountSummary[]) {
  const accountIds = new Set<string>();
  const sourceIds: string[] = [];
  const canRefresh = (account: AccountSummary) => account.enabled && account.secretAvailable
    && !account.recoveryRequired && !account.draining
    && !["requires_reauth", "refreshing"].includes(accountQuotaRefreshState(account));
  for (const member of members) {
    if (member.kind === "source") {
      if (member.secretAvailable) sourceIds.push(member.id);
    } else if (canRefresh(member)) {
      const reader = accountMetadataSources(member, accounts).quota;
      if (canRefresh(reader)) accountIds.add(reader.id);
    }
  }
  return { accountIds: [...accountIds], sourceIds };
}
