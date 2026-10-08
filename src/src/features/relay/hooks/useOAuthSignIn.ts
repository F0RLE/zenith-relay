import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import { forgetAccountLoginDraft, mergeAccountLoginDraft, takeAccountLoginDraft } from "../accountLoginDraft";
import type { OAuthCompletion, OAuthFlow, OAuthFlowEvent } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";
import { captureOperationResult } from "../state/relayOperationModel";

export function useOAuthSignIn(onComplete?: (result: OAuthCompletion) => void | Promise<void>) {
  const { perform } = useRelayState();
  const [flow, setFlow] = useState<OAuthFlow | null>(null);
  const flowRef = useRef<OAuthFlow | null>(null);
  const listenerRef = useRef<ReturnType<typeof relayCommands.onOAuthStatus> | null>(null);
  const handlerRef = useRef<(event: OAuthFlowEvent) => void>(() => undefined);
  const finishRef = useRef<(loginId: string) => Promise<boolean>>(async () => false);
  const latestEventRef = useRef<OAuthFlowEvent | null>(null);
  const onCompleteRef = useRef(onComplete);
  const startingRef = useRef(false);
  const completingRef = useRef(false);
  onCompleteRef.current = onComplete;

  const ensureListener = useCallback(async () => {
    listenerRef.current ??= relayCommands
      .onOAuthStatus((event) => handlerRef.current(event))
      .catch((error) => {
        listenerRef.current = null;
        throw error;
      });
    return listenerRef.current;
  }, []);

  const finish = useCallback(async (loginId: string) => {
    if (completingRef.current) return false;
    completingRef.current = true;
    try {
      const captured = await captureOperationResult(
        (work) => perform("oauth-complete", work, "feedback.accountAdded"),
        async () => {
          const completed = await relayCommands.completeOAuth(loginId);
          const draft = takeAccountLoginDraft(loginId);
          const accountId = completed.account.id;
          if (draft && accountId && (draft.email || draft.phone || draft.password || draft.totpSecret)) {
            try {
              const storedLogin = await relayCommands.revealLocalAccountLogin(accountId);
              await relayCommands.updateAccountLogin({
                accountId,
                ...mergeAccountLoginDraft({
                  email: storedLogin.email ?? "",
                  phone: storedLogin.phone ?? "",
                  password: storedLogin.password ?? "",
                  totpSecret: storedLogin.totpSecret ?? "",
                }, draft),
              });
            } catch {
              // The account is already stored. Notes remain editable on the card.
            }
          }
          return completed;
        },
      );
      if (captured.ok && captured.value) {
        flowRef.current = null;
        setFlow(null);
        await onCompleteRef.current?.(captured.value);
      }
      return captured.ok;
    } catch {
      // A callback or late renderer failure must not leave the sign-in lock
      // held or become an unhandled promise from the native event listener.
      return false;
    } finally {
      completingRef.current = false;
    }
  }, [perform]);
  finishRef.current = finish;

  handlerRef.current = (event) => {
    latestEventRef.current = event;
    const activeFlow = flowRef.current;
    if (!activeFlow || activeFlow.loginId !== event.loginId) return;
    const updatedFlow = { ...activeFlow, status: event.status };
    flowRef.current = updatedFlow;
    setFlow(updatedFlow);
    if (event.status === "callback_received") void finishRef.current(event.loginId).catch(() => undefined);
  };

  const start = useCallback(async (openBrowser = true, accountId?: string, proxyId?: string) => {
    if (startingRef.current) return false;
    startingRef.current = true;
    let captured: { ok: boolean; value: OAuthFlow | undefined } = { ok: false, value: undefined };
    try {
      captured = await captureOperationResult(
        (work) => perform("oauth-start", work),
        async () => {
          await ensureListener();
          return relayCommands.startOAuth(openBrowser, accountId, proxyId);
        },
      );
    } catch {
      captured = { ok: false, value: undefined };
    } finally {
      startingRef.current = false;
    }
    const started = captured.value;
    if (!captured.ok || !started) return false;
    const earlyEvent = latestEventRef.current;
    const startedFlow = earlyEvent?.loginId === started.loginId
      ? { ...started, status: earlyEvent.status }
      : started;
    flowRef.current = startedFlow;
    setFlow(startedFlow);
    if (startedFlow.status === "callback_received") void finishRef.current(startedFlow.loginId).catch(() => undefined);
    return true;
  }, [ensureListener, perform]);

  const cancel = useCallback(async () => {
    const activeFlow = flowRef.current;
    flowRef.current = null;
    setFlow(null);
    if (activeFlow) {
      forgetAccountLoginDraft(activeFlow.loginId);
      try {
        await perform("oauth-cancel", () => relayCommands.cancelOAuth(activeFlow.loginId));
      } catch {
        // Cancellation is best-effort after the dialog has already closed.
      }
    }
  }, [perform]);

  useEffect(() => () => {
    const listener = listenerRef.current;
    if (listener) void listener.then((unlisten) => unlisten()).catch(() => undefined);
  }, []);

  return { flow, start, cancel };
}
