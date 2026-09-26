import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import { sanitizeFeedbackError } from "./feedback";
import {
  runRelayOperation,
  type Feedback,
  type PerformOptions,
} from "./relayOperationModel";

const SUCCESS_FEEDBACK_TIMEOUT_MS = 4_000;
const ERROR_FEEDBACK_TIMEOUT_MS = 60_000;

type Refresh = () => Promise<void>;

/** Own mutation concurrency, progress, and feedback independently of runtime state. */
export function useRelayOperations() {
  const { i18n, t } = useTranslation();
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const operationRevision = useRef(0);

  const performOperation = useCallback((
    id: string,
    work: () => Promise<unknown>,
    refresh: Refresh,
    successKey?: string,
    options?: PerformOptions,
  ) => {
    const revision = ++operationRevision.current;
    setBusy(id);
    setFeedback(null);
    return runRelayOperation({
      work,
      refresh,
      isCurrent: () => revision === operationRevision.current,
      ...(successKey !== undefined ? { successKey } : {}),
      options: {
        ...options,
        onError: (error, key) => {
          // A caller-provided feedback hook is UI code.  It must not be able
          // to turn an already handled command failure into an unhandled
          // rejection (or prevent the diagnostic record from being written).
          try {
            options?.onError?.(error, key);
          } catch {
            // Keep the operation failure isolated from optional UI callbacks.
          }
          void relayCommands.recordFrontendDiagnostic({
            source: "relay-operation",
            operation: id,
            code: error.code,
            message: error.message,
          }).catch(() => undefined);
        },
      },
      resolveError: (cause) => {
        const code = typeof cause === "object" && cause && "code" in cause
          ? String(cause.code)
          : "general";
        const key = i18n.exists(`errors.${code}`) ? `errors.${code}` : "errors.general";
        return { key, error: sanitizeFeedbackError(cause, code, t(key)) };
      },
      setFeedback,
      settle: () => setBusy(null),
    });
  }, [i18n, t]);

  const cancelOperations = useCallback(() => {
    operationRevision.current += 1;
    setBusy(null);
    setFeedback(null);
  }, []);

  const clearFeedback = useCallback(() => setFeedback(null), []);

  const reportErrorFeedback = useCallback((cause: unknown, key: string, fallbackCode: string) => {
    setFeedback({
      kind: "error",
      key,
      error: sanitizeFeedbackError(cause, fallbackCode, t(key)),
    });
  }, [t]);

  useEffect(() => {
    if (!feedback) return;
    const timeout = window.setTimeout(
      clearFeedback,
      feedback.kind === "success" ? SUCCESS_FEEDBACK_TIMEOUT_MS : ERROR_FEEDBACK_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [clearFeedback, feedback]);

  return {
    busy,
    feedback,
    performOperation,
    cancelOperations,
    clearFeedback,
    reportErrorFeedback,
  };
}
