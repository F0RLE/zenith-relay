import { useEffect, useState } from "react";

const URGENT_WINDOW_MS = 60 * 60_000;
const ACCOUNT_TIMESTAMP_OPTIONS = {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
} as const;

/**
 * Refreshes relative account/quota timestamps once a minute, or once a second
 * for the final hour before an event.
 */
export function useRelativeTimeClock(timestamps: readonly (number | null | undefined)[]) {
  const [nowMs, setNowMs] = useState(Date.now());
  const delay = relativeTimeRefreshDelay(timestamps, nowMs);

  useEffect(() => {
    if (delay == null) return;
    const timer = window.setTimeout(() => setNowMs(Date.now()), delay);
    return () => window.clearTimeout(timer);
  }, [delay, nowMs]);

  return nowMs;
}

export function relativeTimeRefreshDelay(timestamps: readonly (number | null | undefined)[], nowMs: number) {
  const upcoming = timestamps.filter((value): value is number => value != null && value > nowMs);
  if (!upcoming.length) return null;
  return upcoming.some((value) => value - nowMs < URGENT_WINDOW_MS) ? 1_000 : 60_000;
}

export function subscriptionExpiryFormatter(locale: string) {
  return new Intl.DateTimeFormat(locale, ACCOUNT_TIMESTAMP_OPTIONS);
}
