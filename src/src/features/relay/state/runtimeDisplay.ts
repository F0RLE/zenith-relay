import type { AccountSummary, RuntimeSnapshot } from "../api/types";

type AccountDisplayName = (accountId?: string | null, fallbackLabel?: string | null) => string | null;

export function projectRuntimeAccountLabels(
  runtime: RuntimeSnapshot | null,
  accountDisplayName: AccountDisplayName,
): RuntimeSnapshot | null {
  if (!runtime) return null;
  let accountsChanged = false;
  const accounts = runtime.accounts.map((account): AccountSummary => {
    const displayName = accountDisplayName(account.id, account.label) ?? account.label;
    if (displayName === account.label && displayName === account.identityHint) return account;
    accountsChanged = true;
    return { ...account, label: displayName, identityHint: displayName };
  });
  return {
    ...runtime,
    accounts: accountsChanged ? accounts : runtime.accounts,
  };
}
