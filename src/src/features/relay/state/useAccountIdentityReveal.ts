import { useEffect, useState, type Dispatch, type SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { AccountSummary, RelayMode } from "../api/types";
import { replaceRevealedAccountIdentities, revealableAccountIds } from "./accountIdentity";

type UseAccountIdentityRevealInput = {
  accounts: readonly AccountSummary[];
  canReveal: boolean;
  identitiesVisible: boolean;
  mode: RelayMode;
  setRevealedIdentities: Dispatch<SetStateAction<Record<string, string>>>;
};

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
    void Promise.allSettled(revealableIds.map((accountId) => mode === "local"
      ? relayCommands.revealLocalAccountIdentity(accountId)
      : relayCommands.revealRemoteAccountIdentity(accountId)))
      .then((results) => {
        if (!active) return;
        const identities = results.flatMap((result) => result.status === "fulfilled" ? [result.value] : []);
        setRevealedIdentities((current) => replaceRevealedAccountIdentities(current, mode, identities));
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
    };
  }, [accountSignature, canReveal, identitiesVisible, mode, setRevealedIdentities]);

  return busy;
}
