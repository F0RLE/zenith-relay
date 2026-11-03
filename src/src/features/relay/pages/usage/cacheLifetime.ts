import type { CacheSessionRecord } from "../../api/types";
import type { UsageRow } from "./usageData";

/** A model name does not establish the selected provider's retention policy. */
export function cacheWriteDurationWindows(
  ttl: string | null | undefined,
): string[] {
  return [...new Set((ttl ?? "")
    .split(",")
    .map((window) => window.trim().toLowerCase())
    .filter((window) => (cacheWindowMs(window) ?? 0) > 0))];
}

const CACHE_WINDOW_MS: Record<string, number> = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000, d: 86_400_000 };

export type CacheLifetimeExpiry = "open" | "elapsed" | "unknown";

export type CacheLifetime = {
  windows: string[];
  windowMs: number | null;
  remainingMs: number | null;
  expiry: CacheLifetimeExpiry;
};

export type CacheTouch = { cacheWriteTtl: string | null; touchedAt: string };

type CacheRequest = Pick<UsageRow, "clientContextId" | "candidateKind" | "candidateKey"
  | "routedModel" | "requestedModel" | "cacheWriteTtl" | "time" | "cachedInputTokens" | "cacheWriteInputTokens">;

/** A miss cannot move the last observed touch; other routes cannot supply it. */
export function cacheTouchForRow(row: CacheRequest, sessions: readonly CacheSessionRecord[]): CacheTouch | null {
  const own = (row.cachedInputTokens ?? 0) > 0 || (row.cacheWriteInputTokens ?? 0) > 0
    ? { cacheWriteTtl: row.cacheWriteTtl, touchedAt: row.time } : null;
  const session = row.clientContextId ? sessions.find((candidate) =>
    candidate.clientContextId === row.clientContextId
    && candidate.candidateKind === row.candidateKind
    && candidate.candidateId === row.candidateKey
    && candidate.model === (row.routedModel ?? row.requestedModel)
    && Number.isFinite(Date.parse(candidate.touchedAt))) : null;
  if (!session) return own;
  if (own && Date.parse(own.touchedAt) > Date.parse(session.touchedAt)) {
    return { touchedAt: own.touchedAt, cacheWriteTtl: own.cacheWriteTtl ?? session.cacheWriteTtl };
  }
  return { cacheWriteTtl: session.cacheWriteTtl, touchedAt: session.touchedAt };
}

/** Mixed windows describe independent prefixes, not one cache with the longest TTL. */
export function cacheLifetime(
  input: CacheTouch,
  nowMs: number,
): CacheLifetime {
  const windows = cacheWriteDurationWindows(input.cacheWriteTtl);
  const windowMs = windows.length === 1 ? cacheWindowMs(windows[0]!) : null;
  const touchedMs = Date.parse(input.touchedAt);
  if (windowMs == null || !Number.isFinite(touchedMs) || !Number.isFinite(nowMs)) {
    return { windows, windowMs: null, remainingMs: null, expiry: "unknown" };
  }
  const remainingMs = Math.max(0, windowMs - Math.max(0, nowMs - touchedMs));
  if (remainingMs > 0) return { windows, windowMs, remainingMs, expiry: "open" };
  return { windows, windowMs, remainingMs: 0, expiry: "elapsed" };
}

function cacheWindowMs(window: string): number | null {
  const match = /^(\d{1,5})(ms|s|m|h|d)$/.exec(window);
  if (!match) return null;
  const amount = Number(match[1]);
  const unit = CACHE_WINDOW_MS[match[2] ?? ""];
  return unit == null ? null : amount * unit;
}
