import type { FeedbackError } from "./feedback";

export type Feedback = { kind: "success" | "error"; key: string; error?: FeedbackError } | null;

export type PerformOptions = {
  /** Keep an operation error local to the surface that initiated it. */
  reportError?: boolean;
  onError?: (error: FeedbackError, key: string) => void;
};

export type ResolvedOperationError = {
  key: string;
  error: FeedbackError;
};

export type RelayOperationInput = {
  work: () => Promise<unknown>;
  refresh: () => Promise<void>;
  isCurrent: () => boolean;
  successKey?: string;
  options?: PerformOptions;
  resolveError: (error: unknown) => ResolvedOperationError;
  setFeedback: (feedback: Exclude<Feedback, null>) => void;
  settle: () => void;
};

/**
 * Execute one mutation without owning React state. The caller supplies the
 * operation-revision guard so an older completion cannot refresh or overwrite
 * feedback for a newer operation.
 */
export async function runRelayOperation({
  work,
  refresh,
  isCurrent,
  successKey,
  options,
  resolveError,
  setFeedback,
  settle,
}: RelayOperationInput): Promise<boolean> {
  try {
    await work();
    if (!isCurrent()) return false;
    await refresh();
    if (!isCurrent()) return false;
    if (successKey) setFeedback({ kind: "success", key: successKey });
    return true;
  } catch (cause) {
    if (!isCurrent()) return false;
    const resolved = resolveError(cause);
    options?.onError?.(resolved.error, resolved.key);
    if (options?.reportError !== false) {
      setFeedback({ kind: "error", key: resolved.key, error: resolved.error });
    }
    return false;
  } finally {
    if (isCurrent()) settle();
  }
}
