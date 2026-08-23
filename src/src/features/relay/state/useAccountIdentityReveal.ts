import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { AccountSummary, RelayMode, RevealedAccountIdentity } from "../api/types";
import { replaceRevealedAccountIdentities, revealableAccountIds } from "./accountIdentity";

type UseAccountIdentityRevealInput = {
  accounts: readonly AccountSummary[];
  canReveal: boolean;
  identitiesVisible: boolean;
  mode: RelayMode;
  setRevealedIdentities: Dispatch<SetStateAction<Record<string, string>>>;
};

type AccountIdentityRevealRun = {
  accountIds: readonly string[];
  isActive: () => boolean;
  reveal: (accountId: string) => Promise<RevealedAccountIdentity>;
  onRevealed: (identities: RevealedAccountIdentity[]) => void;
  onComplete: () => void;
};

export async function runAccountIdentityReveal({
  accountIds,
  isActive,
  reveal,
  onRevealed,
  onComplete,
}: AccountIdentityRevealRun) {
  const results = await Promise.allSettled(accountIds.map(reveal));
  if (!isActive()) return;
  onRevealed(results.flatMap((result) => result.status === "fulfilled" ? [result.value] : []));
  if (isActive()) onComplete();
}

export function useAccountIdentityReveal({
  accounts,
  canReveal,
  identitiesVisible,
  mode,
  setRevealedIdentities,
}: UseAccountIdentityRevealInput) {
  const [busy, setBusy] = useState(false);
  const accountSignature = revealableAccountIds(accounts, canReveal).join("\0");

  useEffect(() => {
    if (!identitiesVisible) {
      setBusy(false);
      return;
    }
    if (!canReveal || !accountSignature) {
      setRevealedIdentities((current) => replaceRevealedAccountIdentities(current, mode, []));
      setBusy(false);
      return;
    }
    let active = true;
    const revealableIds = accountSignature.split("\0");
    setBusy(true);
    void runAccountIdentityReveal({
      accountIds: revealableIds,
      isActive: () => active,
      reveal: (accountId) => mode === "local"
        ? relayCommands.revealLocalAccountIdentity(accountId)
        : relayCommands.revealRemoteAccountIdentity(accountId),
      onRevealed: (identities) => {
        setRevealedIdentities((current) => replaceRevealedAccountIdentities(current, mode, identities));
      },
      onComplete: () => setBusy(false),
    });
    return () => {
      active = false;
    };
  }, [accountSignature, canReveal, identitiesVisible, mode, setRevealedIdentities]);

  return busy;
}
