import { useEffect, useState } from "react";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { OAuthAccountSetupDialog, OAuthDialog } from "../pages/connections/OAuthDialogs";
import { rememberSignInProxyId } from "../pages/connections/signInProxyPreference";
import { useRelayState } from "../state/RelayStateProvider";
import { useOAuthSignIn } from "./useOAuthSignIn";

/** Sign-in, reauthentication, and post-login setup for Connections. */
export function useAccountSignIn() {
  const { mode } = useRelayState();
  const [accountId, setAccountId] = useState<string | null>(null);
  const [reauthenticatingId, setReauthenticatingId] = useState<string | null>(null);
  const [proxyId, setProxyId] = useState<string | undefined>();
  const oauth = useOAuthSignIn((result) => {
    setAccountId(reauthenticatingId === result.account.id ? null : result.account.id);
    setReauthenticatingId(null);
  });

  useEffect(() => {
    setAccountId(null);
    setReauthenticatingId(null);
    setProxyId(undefined);
  }, [mode]);

  const start = () => {
    setAccountId(null);
    setReauthenticatingId(null);
    setProxyId(undefined);
    void oauth.start(false);
  };
  const reauthenticate = (account: AccountSummary) => {
    setAccountId(null);
    setReauthenticatingId(account.id);
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
    {accountId ? <OAuthAccountSetupDialog
      accountId={accountId}
      preserveProxy={Boolean(proxyId)}
      onClose={() => { setAccountId(null); setProxyId(undefined); }}
    /> : null}
  </>;

  return { start, reauthenticate, dialogs };
}
