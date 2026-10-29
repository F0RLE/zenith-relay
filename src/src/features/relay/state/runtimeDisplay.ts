import type { AccountSummary, RuntimeSnapshot } from "../api/types";

type AccountDisplayName = (accountId?: string | null, fallbackLabel?: string | null) => string | null;

/** Retain only the current projection, not one copy per activity snapshot. */
export function createRuntimeAccountLabelProjector() {
  let previousAccounts: AccountSummary[] | undefined;
  let previousDisplayName: AccountDisplayName | undefined;
  let projectedAccounts: AccountSummary[] | undefined;

  return (runtime: RuntimeSnapshot | null, accountDisplayName: AccountDisplayName): RuntimeSnapshot | null => {
    if (!runtime) {
      previousAccounts = undefined;
      previousDisplayName = undefined;
      projectedAccounts = undefined;
      return null;
    }
    if (runtime.accounts !== previousAccounts || accountDisplayName !== previousDisplayName) {
      projectedAccounts = projectRuntimeAccountLabels(runtime, accountDisplayName)!.accounts;
      previousAccounts = runtime.accounts;
      previousDisplayName = accountDisplayName;
    }
    return projectedAccounts === runtime.accounts ? runtime : { ...runtime, accounts: projectedAccounts! };
  };
}

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
