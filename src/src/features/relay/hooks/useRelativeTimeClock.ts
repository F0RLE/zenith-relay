import { useEffect, useState } from "react";

const URGENT_WINDOW_MS = 60 * 60_000;
const ACCOUNT_TIMESTAMP_OPTIONS = {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
} as const;
let timestampFormatter: { locale: string; value: Intl.DateTimeFormat } | undefined;

/**
 * Refreshes relative account/quota timestamps once a minute, or once a second
 * for the final hour before an event.
 */
export function useRelativeTimeClock(timestamps: readonly (number | null | undefined)[]) {
  const [nowMs, setNowMs] = useState(Date.now);
  const timestampKey = timestamps.map((timestampValue) => timestampValue ?? "").join(":");

  useEffect(() => {
    let timer: number | undefined;
    const refresh = () => {
      window.clearTimeout(timer);
      if (document.visibilityState !== "visible") return;
      const currentTime = Date.now();
      setNowMs(currentTime);
      const delay = relativeTimeRefreshDelay(timestamps, currentTime);
      if (delay != null) timer = window.setTimeout(refresh, delay);
    };
    refresh();
    // Resume from wall time, even if every deadline expired while hidden.
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [timestampKey]);

  return nowMs;
}

export function relativeTimeRefreshDelay(timestamps: readonly (number | null | undefined)[], nowMs: number) {
  let delay: number | null = null;
  for (const timestamp of timestamps) {
    if (timestamp == null || !(timestamp > nowMs)) continue;
    if (timestamp - nowMs < URGENT_WINDOW_MS) return 1_000;
    delay = 60_000;
  }
  return delay;
}

export function secondsUntil(deadlineMs: number, nowMs: number) {
  return Math.max(0, Math.ceil((deadlineMs - nowMs) / 1_000));
}

export function subscriptionExpiryFormatter(locale: string) {
  if (timestampFormatter?.locale !== locale) {
    timestampFormatter = { locale, value: new Intl.DateTimeFormat(locale, ACCOUNT_TIMESTAMP_OPTIONS) };
  }
  return timestampFormatter.value;
}
