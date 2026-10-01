import type { FeedbackError } from "./feedback";

export type Feedback = { kind: "success" | "error"; key: string; error?: FeedbackError } | null;

export type PerformOptions = {
  /** Finish a dependent step before refreshing, only while the operation is current. */
  afterWork?: () => Promise<unknown>;
  /** Unlock the UI after the command succeeds and refresh without holding the busy lock. */
  backgroundRefresh?: boolean;
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
    if (options?.afterWork) {
      await options.afterWork();
      if (!isCurrent()) return false;
    }
    if (options?.backgroundRefresh) {
      if (successKey) setFeedback({ kind: "success", key: successKey });
      settle();
      if (!isCurrent()) return true;
      try {
        await refresh();
      } catch (cause) {
        if (!isCurrent()) return true;
        const resolved = resolveError(cause);
        options.onError?.(resolved.error, resolved.key);
        if (options.reportError !== false) {
          setFeedback({ kind: "error", key: resolved.key, error: resolved.error });
        }
        return false;
      }
      return true;
    }
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

/**
 * Keep the value returned by work. `ok` is still the operation result, so a
 * later refresh failure leaves the value in place. `value` stays undefined
 * until work returns; a returned null stays null.
 */
export async function captureOperationResult<T>(
  run: (work: () => Promise<unknown>) => Promise<boolean>,
  work: () => Promise<T>,
): Promise<{ ok: boolean; value: T | undefined }> {
  let value: T | undefined;
  let captured = false;
  const ok = await run(async () => {
    value = await work();
    captured = true;
  });
  return { ok, value: captured ? value : undefined };
}
