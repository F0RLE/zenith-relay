import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import { forgetAccountLoginDraft, mergeAccountLoginDraft, takeAccountLoginDraft } from "../accountLoginDraft";
import type { OAuthClientKind, OAuthCompletion, OAuthFlow, OAuthFlowEvent } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";
import { captureOperationResult } from "../state/relayOperationModel";

export function useOAuthSignIn(onComplete?: (result: OAuthCompletion) => void | Promise<void>) {
  const { perform } = useRelayState();
  const [flow, setFlow] = useState<OAuthFlow | null>(null);
  const [starting, setStarting] = useState(false);
  const flowRef = useRef<OAuthFlow | null>(null);
  const listenerRef = useRef<ReturnType<typeof relayCommands.onOAuthStatus> | null>(null);
  const handlerRef = useRef<(event: OAuthFlowEvent) => void>(() => undefined);
  const finishRef = useRef<(loginId: string) => Promise<boolean>>(async () => false);
  const latestEventRef = useRef<OAuthFlowEvent | null>(null);
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
    completingRef.current = true;
    try {
      const captured = await captureOperationResult(
        (work) => perform("oauth-complete", work, "feedback.accountAdded", { backgroundRefresh: true }),
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
      // The command may finish while a newer UI operation replaces its
      // feedback. Its returned account still proves that sign-in completed.
      if (captured.value && mountedRef.current && lifecycleRef.current === lifecycle) {
        flowRef.current = null;
        setFlow(null);
        await onCompleteRef.current?.(captured.value);
      }
      return Boolean(captured.value);
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
    if (!mountedRef.current) return;
    latestEventRef.current = event;
    const activeFlow = flowRef.current;
    if (startingRef.current || !activeFlow || activeFlow.loginId !== event.loginId) return;
    const updatedFlow = { ...activeFlow, status: event.status };
    flowRef.current = updatedFlow;
    setFlow(updatedFlow);
    if (event.status === "callback_received") void finishRef.current(event.loginId).catch(() => undefined);
  };

  const start = useCallback(async (openBrowser = true, accountId?: string, proxyId?: string, clientKind?: OAuthClientKind) => {
    if (!mountedRef.current || startingRef.current || completingRef.current || flowRef.current?.status === "callback_received") return false;
    const lifecycle = lifecycleRef.current;
    const isCurrent = () => mountedRef.current && lifecycleRef.current === lifecycle;
    const previous = flowRef.current;
    const requestedClientKind = clientKind ?? previous?.clientKind;
    let previousCancelled = false;
    startingRef.current = true;
    setStarting(true);
    if (previous && requestedClientKind && requestedClientKind !== previous.clientKind) {
      // Keep the dialog mounted and show the requested method immediately.
      // The old flow remains blocked until the replacement is ready.
      setFlow({ ...previous, clientKind: requestedClientKind });
    }
    let captured: { ok: boolean; value: OAuthFlow | undefined } = { ok: false, value: undefined };
    try {
      captured = await captureOperationResult(
        (work) => perform("oauth-start", work, undefined, { backgroundRefresh: true }),
        async () => {
          await ensureListener();
          if (!isCurrent()) return undefined;
          if (previous) {
            await relayCommands.cancelOAuth(previous.loginId);
            previousCancelled = true;
            flowRef.current = null;
            forgetAccountLoginDraft(previous.loginId);
          }
          if (!isCurrent()) return undefined;
          return relayCommands.startOAuth(openBrowser, accountId, proxyId, clientKind);
        },
      );
    } catch {
      captured = { ok: false, value: undefined };
    } finally {
      startingRef.current = false;
      if (mountedRef.current) setStarting(false);
    }
    const started = captured.value;
    if (!started) {
      if (previous && previousCancelled && isCurrent()) {
        // The previous flow has already been cancelled, so do not present it
        // as usable again. Keep the shell mounted with an explicit failure.
        const failedFlow = { ...previous, ...(requestedClientKind ? { clientKind: requestedClientKind } : {}), status: "failed" as const };
        setFlow(failedFlow);
      } else if (previous && isCurrent()) {
        flowRef.current = previous;
        setFlow(previous);
        const earlyEvent = latestEventRef.current;
        if (earlyEvent?.loginId === previous.loginId) handlerRef.current(earlyEvent);
      }
      return false;
    }
    if (!isCurrent()) {
      forgetAccountLoginDraft(started.loginId);
      await relayCommands.cancelOAuth(started.loginId).catch(() => undefined);
      return false;
    }
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
