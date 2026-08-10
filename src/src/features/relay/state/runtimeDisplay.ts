import type { AccountSummary, RuntimeSnapshot } from "../api/types";

type AccountDisplayName = (accountId?: string | null, fallbackLabel?: string | null) => string | null;

export function projectRuntimeAccountLabels(
  runtime: RuntimeSnapshot | null,
  accountDisplayName: AccountDisplayName,
): RuntimeSnapshot | null {
  if (!runtime) return null;
  return {
    ...runtime,
    accounts: runtime.accounts.map((account): AccountSummary => {
      const displayName = accountDisplayName(account.id, account.label) ?? account.label;
      return { ...account, label: displayName, identityHint: displayName };
    }),
  };
}
