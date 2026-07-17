import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { OAuthCompletion, OAuthFlow, OAuthFlowEvent } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";

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
    const completed: { current: OAuthCompletion | null } = { current: null };
    const ok = await perform("oauth-complete", async () => {
      completed.current = await relayCommands.completeOAuth(loginId);
    }, "feedback.accountAdded");
    completingRef.current = false;
    if (ok && completed.current) {
      flowRef.current = null;
      setFlow(null);
      await onCompleteRef.current?.(completed.current);
    }
    return ok;
  }, [perform]);
  finishRef.current = finish;

  handlerRef.current = (event) => {
    latestEventRef.current = event;
    const current = flowRef.current;
    if (!current || current.loginId !== event.loginId) return;
    const next = { ...current, status: event.status };
    flowRef.current = next;
    setFlow(next);
    if (event.status === "callback_received") void finishRef.current(event.loginId);
  };

  const start = useCallback(async () => {
    if (startingRef.current) return false;
    startingRef.current = true;
    const result: { current: OAuthFlow | null } = { current: null };
    const ok = await perform("oauth-start", async () => {
      await ensureListener();
      result.current = await relayCommands.startOAuth();
    });
    startingRef.current = false;
    const started = result.current;
    if (!ok || !started) return false;
    const earlyEvent = latestEventRef.current;
    const next = earlyEvent?.loginId === started.loginId
      ? { ...started, status: earlyEvent.status }
      : started;
    flowRef.current = next;
    setFlow(next);
    if (next.status === "callback_received") void finishRef.current(next.loginId);
    return true;
  }, [ensureListener, perform]);

  const cancel = useCallback(async () => {
    const current = flowRef.current;
    flowRef.current = null;
    setFlow(null);
    if (current) await perform("oauth-cancel", () => relayCommands.cancelOAuth(current.loginId));
  }, [perform]);

  useEffect(() => () => {
    const listener = listenerRef.current;
    if (listener) void listener.then((unlisten) => unlisten()).catch(() => undefined);
  }, []);

  return { flow, start, cancel };
}
