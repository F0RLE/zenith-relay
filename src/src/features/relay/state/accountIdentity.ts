import type { AccountSummary, RelayMode, RevealedAccountIdentity } from "../api/types";

export type AccountIdentityIndex = {
  uniqueById: ReadonlyMap<string, AccountSummary | null>;
  uniqueByLabel: ReadonlyMap<string, AccountSummary | null>;
};

type DisplayAccountIdentityInput = {
  index: AccountIdentityIndex;
  accountId?: string | null;
  fallbackLabel?: string | null;
  identitiesVisible: boolean;
  canReveal: boolean;
  mode: RelayMode;
  revealedIdentities: Readonly<Record<string, string>>;
};

export function buildAccountIdentityIndex(accounts: readonly AccountSummary[]): AccountIdentityIndex {
  const uniqueById = new Map<string, AccountSummary | null>();
  const uniqueByLabel = new Map<string, AccountSummary | null>();
  for (const account of accounts) {
    uniqueById.set(account.id, uniqueById.has(account.id) ? null : account);
    uniqueByLabel.set(account.label, uniqueByLabel.has(account.label) ? null : account);
  }
  return { uniqueById, uniqueByLabel };
}

export function revealableAccountIds(accounts: readonly AccountSummary[], canReveal: boolean): string[] {
  return canReveal ? accounts.filter((account) => account.secretAvailable).map((account) => account.id) : [];
}

export function replaceRevealedAccountIdentities(
  current: Readonly<Record<string, string>>,
  mode: RelayMode,
  identities: readonly RevealedAccountIdentity[],
): Record<string, string> {
  const prefix = `${mode}:`;
  const next = Object.fromEntries(Object.entries(current).filter(([key]) => !key.startsWith(prefix)));
  for (const identity of identities) next[`${prefix}${identity.accountId}`] = identity.identity;
  return next;
}

export function displayAccountIdentity({
  index,
  accountId,
  fallbackLabel,
  identitiesVisible,
  canReveal,
  mode,
  revealedIdentities,
}: DisplayAccountIdentityInput): string | null {
  const account = accountId
    ? index.uniqueById.get(accountId) ?? null
    : fallbackLabel ? index.uniqueByLabel.get(fallbackLabel) ?? null : null;
  if (!account) return fallbackLabel ?? null;
  const maskedIdentity = account.identityHint.trim() || account.label;
  if (!identitiesVisible || !canReveal || !account.secretAvailable) return maskedIdentity;
  return revealedIdentities[`${mode}:${account.id}`] ?? maskedIdentity;
}
