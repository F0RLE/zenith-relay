import { useEffect, useRef, useState } from "react";
import type { AccountSummary, OAuthClientKind } from "../api/types";
import { OAuthDialog } from "../pages/connections/OAuthDialogs";
import { rememberSignInProxyId } from "../pages/connections/signInProxyPreference";
import { useRelayState } from "../state/RelayStateProvider";
import { useOAuthSignIn } from "./useOAuthSignIn";
import { relayCommands } from "../api/commands";

/** Sign-in and reauthentication; optional setup remains on the account card. */
export function useAccountSignIn() {
  const { mode, perform } = useRelayState();
  const [proxyId, setProxyId] = useState<string | undefined>();
  const routeTarget = useRef<string | undefined>(undefined);
  const oauth = useOAuthSignIn(async (completed) => {
    setProxyId(undefined);
    const target = routeTarget.current;
    routeTarget.current = undefined;
    if (target && target !== completed.account.id) {
      await perform("account-route-link", () => relayCommands.selectAccountRoute(target, completed.account.id, true), undefined, { backgroundRefresh: true });
    }
  });

  useEffect(() => {
    setProxyId(undefined);
    routeTarget.current = undefined;
  }, [mode]);

  const start = () => {
    routeTarget.current = undefined;
    setProxyId(undefined);
    void oauth.start(false);
  };
  const reauthenticate = (account: AccountSummary, automatic = false) => {
    routeTarget.current = undefined;
    setProxyId(undefined);
    void oauth.start(automatic, account.id, undefined, undefined, automatic);
  };
  const useProxy = (nextProxyId: string) => {
    rememberSignInProxyId(nextProxyId);
    setProxyId(nextProxyId);
    void oauth.start(true, undefined, nextProxyId, oauth.flow?.clientKind ?? "codex");
  };
  const useClient = (clientKind: OAuthClientKind) => {
    void oauth.start(false, undefined, proxyId, clientKind);
  };
  const startRoute = (account: AccountSummary, clientKind: OAuthClientKind, automatic = false) => {
    routeTarget.current = account.id;
    setProxyId(undefined);
    void oauth.start(automatic, undefined, undefined, clientKind, automatic, automatic ? account.id : undefined);
  };
  const dialogs = <>
    {oauth.flow ? <OAuthDialog
      flow={oauth.flow}
      notesAccountId={routeTarget.current}
      starting={oauth.starting}
      onCancel={async () => { routeTarget.current = undefined; await oauth.cancel(); }}
      onUseProxy={oauth.flow.targetAccountId ? undefined : useProxy}
      onUseClient={oauth.flow.targetAccountId || routeTarget.current ? undefined : useClient}
    /> : null}
  </>;

  return { start, startRoute, reauthenticate, dialogs };
}
