import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import { forgetAccountLoginDraft, mergeAccountLoginDraft, takeAccountLoginDraft } from "../accountLoginDraft";
import type { OAuthClientKind, OAuthCompletion, OAuthFlow, OAuthFlowEvent } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";
import { applyOAuthFlowEvent } from "../oauthFlowEvent";

export function useOAuthSignIn(onComplete?: (result: OAuthCompletion) => void | Promise<void>) {
  const { perform } = useRelayState();
  const [flow, setFlow] = useState<OAuthFlow | null>(null);
  const [starting, setStarting] = useState(false);
  const flowRef = useRef<OAuthFlow | null>(null);
  const listenerRef = useRef<ReturnType<typeof relayCommands.onOAuthStatus> | null>(null);
  const handlerRef = useRef<(event: OAuthFlowEvent) => void>(() => undefined);
  const finishRef = useRef<(loginId: string) => Promise<boolean>>(async () => false);
  const earlyEventsRef = useRef(new Map<string, OAuthFlowEvent>());
  const onCompleteRef = useRef(onComplete);
  const startingRef = useRef(false);
  const completingRef = useRef(false);
  const mountedRef = useRef(true);
  const lifecycleRef = useRef(0);
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
    if (!mountedRef.current || completingRef.current || flowRef.current?.loginId !== loginId) return false;
    const lifecycle = lifecycleRef.current;
    let saved = false;
    completingRef.current = true;
    try {
      await perform(
        "oauth-complete",
        async () => {
          const completed = await relayCommands.completeOAuth(loginId);
          saved = true;
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
          // Link a route before the snapshot can filter or unmount its original card.
          await onCompleteRef.current?.(completed);
        },
        "feedback.accountAdded", { backgroundRefresh: true },
      );
      return saved;
    } catch {
      // A post-save failure cannot undo the completed login or consume it again.
      return saved;
    } finally {
      completingRef.current = false;
      const activeFlow = flowRef.current;
      if (mountedRef.current && lifecycleRef.current === lifecycle && activeFlow?.loginId === loginId) {
        // A saved login closes even when linking fails; its error remains visible.
        // Only a failed save leaves the sign-in flow open for recovery.
        const nextFlow = saved ? null : { ...activeFlow, status: "failed" as const };
        flowRef.current = nextFlow;
        setFlow(nextFlow);
      }
    }
  }, [perform]);
  finishRef.current = finish;

  handlerRef.current = (event) => {
    if (!mountedRef.current) return;
    if (startingRef.current) {
      const previous = earlyEventsRef.current.get(event.loginId);
      earlyEventsRef.current.set(event.loginId, previous ? applyOAuthFlowEvent(previous, event) : event);
      return;
    }
    const activeFlow = flowRef.current;
    if (!activeFlow || activeFlow.loginId !== event.loginId) return;
    const updatedFlow = applyOAuthFlowEvent(activeFlow, event);
    flowRef.current = updatedFlow;
    setFlow(updatedFlow);
    if (event.automaticLogin === undefined && event.diagnostic === undefined && event.status === "callback_received") void finishRef.current(event.loginId).catch(() => undefined);
  };

  const start = useCallback(async (openBrowser = true, accountId?: string, proxyId?: string, clientKind?: OAuthClientKind, automaticLogin = false, notesAccountId?: string) => {
    if (!mountedRef.current || startingRef.current || completingRef.current || flowRef.current?.status === "callback_received") return false;
    const lifecycle = lifecycleRef.current;
    const isCurrent = () => mountedRef.current && lifecycleRef.current === lifecycle;
    const previous = flowRef.current;
    const requestedClientKind = clientKind ?? previous?.clientKind;
    let previousCancelled = false;
    earlyEventsRef.current.clear();
    startingRef.current = true;
    setStarting(true);
    if (previous && requestedClientKind && requestedClientKind !== previous.clientKind) {
      // Keep the dialog mounted and show the requested method immediately.
      // The old flow remains blocked until the replacement is ready.
      setFlow({ ...previous, clientKind: requestedClientKind });
    }
    let started: OAuthFlow | undefined;
    try {
      await perform(
        "oauth-start",
        async () => {
          await ensureListener();
          if (!isCurrent()) return;
          if (previous) {
            await relayCommands.cancelOAuth(previous.loginId);
            previousCancelled = true;
            flowRef.current = null;
            forgetAccountLoginDraft(previous.loginId);
          }
          if (!isCurrent()) return;
          const issued = await relayCommands.startOAuth(openBrowser, accountId, proxyId, clientKind, automaticLogin, notesAccountId);
          started = automaticLogin ? { ...issued, automaticLogin: issued.automaticLogin ?? true } : issued;
        },
        undefined, { backgroundRefresh: true },
      );
    } catch {
      // Retain an issued attempt even if renderer feedback fails afterwards.
    } finally {
      startingRef.current = false;
      if (mountedRef.current) setStarting(false);
    }
    const earlyEvents = earlyEventsRef.current;
    earlyEventsRef.current = new Map();
    if (!started) {
      if (previous && previousCancelled && isCurrent()) {
        // The previous flow has already been cancelled, so do not present it
        // as usable again. Keep the shell mounted with an explicit failure.
        const failedFlow = { ...previous, ...(requestedClientKind ? { clientKind: requestedClientKind } : {}), status: "failed" as const };
        setFlow(failedFlow);
      } else if (previous && isCurrent()) {
        flowRef.current = previous;
        setFlow(previous);
        const earlyEvent = earlyEvents.get(previous.loginId);
        if (earlyEvent) handlerRef.current(earlyEvent);
      }
      return false;
    }
    if (!isCurrent()) {
      forgetAccountLoginDraft(started.loginId);
      await relayCommands.cancelOAuth(started.loginId).catch(() => undefined);
      return false;
    }
    const earlyEvent = earlyEvents.get(started.loginId);
    const startedFlow = earlyEvent
      ? { ...started, ...earlyEvent }
      : started;
    flowRef.current = startedFlow;
    setFlow(startedFlow);
    if (startedFlow.status === "callback_received") void finishRef.current(startedFlow.loginId).catch(() => undefined);
    return true;
  }, [ensureListener, perform]);

  const cancel = useCallback(async () => {
    if (completingRef.current) return;
    lifecycleRef.current += 1;
    const activeFlow = flowRef.current;
    flowRef.current = null;
    if (mountedRef.current) setFlow(null);
    if (activeFlow) {
      forgetAccountLoginDraft(activeFlow.loginId);
      try {
        await perform("oauth-cancel", () => relayCommands.cancelOAuth(activeFlow.loginId));
      } catch {
        // Cancellation is best-effort after the dialog has already closed.
      }
    }
  }, [perform]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      lifecycleRef.current += 1;
      const listener = listenerRef.current;
      listenerRef.current = null;
      if (listener) void listener.then((unlisten) => unlisten()).catch(() => undefined);
      const activeFlow = flowRef.current;
      flowRef.current = null;
      if (activeFlow && !completingRef.current) {
        forgetAccountLoginDraft(activeFlow.loginId);
        void relayCommands.cancelOAuth(activeFlow.loginId).catch(() => undefined);
      }
    };
  }, []);

  return { flow, starting, start, cancel };
}
