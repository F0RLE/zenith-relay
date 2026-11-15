import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { AccountTotpPreview } from "../api/types";

export function useTotpPreview(secret: string) {
  const [result, setResult] = useState<{ secret: string; value: AccountTotpPreview } | null>(null);
  const latestSecret = useRef(secret);
  const refreshRef = useRef<() => Promise<AccountTotpPreview | null>>(async () => null);
  latestSecret.current = secret;

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    let pending: Promise<AccountTotpPreview | null> | null = null;
    let retryDelay = 1_000;
    const current = () => !cancelled && latestSecret.current === secret;
    const refresh = (): Promise<AccountTotpPreview | null> => {
      if (!current() || !secret.trim() || document.visibilityState !== "visible") return Promise.resolve(null);
      if (pending) return pending;
      window.clearTimeout(timer);
      setResult((previous) => previous?.value.expiresAtMs != null && previous.value.expiresAtMs > Date.now() ? previous : null);
      let delay: number | null = null;
      pending = Promise.resolve()
        .then(() => current() ? relayCommands.previewTotpCode(secret) : null)
        .then((preview) => {
          if (!current() || !preview) return null;
          const now = Date.now();
          if (!preview.code || preview.expiresAtMs == null || preview.expiresAtMs <= now) {
            setResult(null);
            // Invalid input needs an edit; an expired response needs a fresh request.
            if (preview.code) delay = retryDelay;
            return null;
          }
          retryDelay = 1_000;
          delay = Math.max(250, preview.expiresAtMs - now + 50);
          setResult({ secret, value: preview });
          return preview;
        })
        .catch(() => {
          if (current()) setResult(null);
          delay = retryDelay;
          retryDelay = Math.min(30_000, retryDelay * 2);
          return null;
        })
        .finally(() => {
          pending = null;
          if (current() && document.visibilityState === "visible" && delay != null) {
            timer = window.setTimeout(() => void refresh(), delay);
          }
        });
      return pending;
    };
    const resume = () => {
      window.clearTimeout(timer);
      if (document.visibilityState === "visible") void refresh();
      else setResult(null);
    };
    setResult(null);
    refreshRef.current = refresh;
    void refresh();
    document.addEventListener("visibilitychange", resume);
    window.addEventListener("focus", resume);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", resume);
      window.removeEventListener("focus", resume);
    };
  }, [secret]);

  const refresh = useCallback(() => refreshRef.current(), []);
  return { preview: result?.secret === secret ? result.value : null, refresh };
}
