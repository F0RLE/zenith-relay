import { useEffect, useState } from "react";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { OAuthDialog } from "../pages/connections/OAuthDialogs";
import { rememberSignInProxyId } from "../pages/connections/signInProxyPreference";
import { useRelayState } from "../state/RelayStateProvider";
import { useOAuthSignIn } from "./useOAuthSignIn";

/** Sign-in and reauthentication; optional setup remains on the account card. */
export function useAccountSignIn() {
  const { mode } = useRelayState();
  const [proxyId, setProxyId] = useState<string | undefined>();
  const oauth = useOAuthSignIn(() => setProxyId(undefined));

  useEffect(() => {
    setProxyId(undefined);
  }, [mode]);

  const start = () => {
    setProxyId(undefined);
    void oauth.start(false);
  };
  const reauthenticate = (account: AccountSummary) => {
    setProxyId(undefined);
    void oauth.start(false, account.id);
  };
  const useProxy = (nextProxyId: string) => {
    rememberSignInProxyId(nextProxyId);
    setProxyId(nextProxyId);
    void oauth.start(true, undefined, nextProxyId, oauth.flow?.clientKind ?? "codex");
  };
  const useClient = (clientKind: OAuthClientKind) => {
    void oauth.start(false, undefined, proxyId, clientKind);
  };
  const dialogs = <>
    {oauth.flow ? <OAuthDialog
      flow={oauth.flow}
      starting={oauth.starting}
      onCancel={oauth.cancel}
      onUseProxy={oauth.flow.targetAccountId ? undefined : useProxy}
      onUseClient={oauth.flow.targetAccountId ? undefined : useClient}
    /> : null}
  </>;

  return { start, reauthenticate, dialogs };
}
