import type { DocumentedCacheRetentionMinimum } from "../../api/types";

export function documentedCacheRetentionMinimum(
  model: string | null | undefined,
  cachedInputTokens: number | null | undefined,
  cacheWriteInputTokens: number | null | undefined,
  reportedCacheWriteTtl?: string | null,
): DocumentedCacheRetentionMinimum | null {
  if (reportedCacheWriteTtl) return null;
  if ((cachedInputTokens ?? 0) <= 0 && (cacheWriteInputTokens ?? 0) <= 0) return null;
  return isGpt56OrLater(model) ? "30m" : null;
}

/** Provider-reported cache-write windows, or the documented OpenAI minimum when usage omits one. */
export function cacheWriteDurationWindows(
  ttl: string | null | undefined,
  documented: DocumentedCacheRetentionMinimum | null,
): string[] {
  const reported = (ttl ?? "")
    .split(",")
    .map((window) => window.trim().toLowerCase())
    .filter((window) => /^\d{1,5}(?:ms|s|m|h|d)$/.test(window));
  if (reported.length) return reported;
  return documented === "30m" ? ["30m"] : [];
}

function isGpt56OrLater(model: string | null | undefined): boolean {
  if (!model) return false;
  return model.split(/[/:]/).some((part) => isGpt56OrLaterComponent(part));
}

function isGpt56OrLaterComponent(component: string): boolean {
  const match = /^gpt-(\d+)(?:\.(\d+))?(.*)$/i.exec(component.trim());
  if (!match) return false;
  const major = Number(match[1]);
  const minor = Number(match[2] ?? 0);
  const suffix = match[3] ?? "";
  if (suffix !== "" && suffix[0] !== "." && suffix[0] !== "-") return false;
  if (suffix.startsWith(".") && !/^\.\d+(?:-|\.|$)/.test(suffix)) return false;
  return major >= 6 || (major === 5 && minor >= 6);
}

const CACHE_WINDOW_MS: Record<string, number> = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000, d: 86_400_000 };

export type CacheLifetimeExpiry = "open" | "minimum_elapsed" | "elapsed" | "unknown";

export type CacheLifetime = {
  windows: string[];
  windowMs: number | null;
  remainingMs: number | null;
  expiry: CacheLifetimeExpiry;
};

/** Estimate from the last cache write or read. A reported window wins; GPT-5.6+ falls back to a 30-minute minimum. */
export function cacheLifetime(
  input: { model: string | null; cacheWriteTtl: string | null; touchedAt: string },
  nowMs: number,
): CacheLifetime {
  const documented = documentedCacheRetentionMinimum(input.model, 1, 0, input.cacheWriteTtl);
  const windows = cacheWriteDurationWindows(input.cacheWriteTtl, documented);
  const durations = windows.map(cacheWindowMs).filter((durationValue): durationValue is number => durationValue != null);
  const windowMs = durations.length ? Math.max(...durations) : null;
  const touchedMs = Date.parse(input.touchedAt);
  if (windowMs == null || !Number.isFinite(touchedMs)) {
    return { windows, windowMs: null, remainingMs: null, expiry: "unknown" };
  }
  const remainingMs = Math.max(0, windowMs - (nowMs - touchedMs));
  const openAiMinimum = windows.length === 1 && windows[0] === "30m" && (documented === "30m" || isGpt56OrLater(input.model));
  if (remainingMs > 0) return { windows, windowMs, remainingMs, expiry: "open" };
  return { windows, windowMs, remainingMs: 0, expiry: openAiMinimum ? "minimum_elapsed" : "elapsed" };
}

function cacheWindowMs(window: string): number | null {
  const match = /^(\d{1,5})(ms|s|m|h|d)$/.exec(window);
  if (!match) return null;
  const amount = Number(match[1]);
  const unit = CACHE_WINDOW_MS[match[2] ?? ""];
  return unit == null ? null : amount * unit;
}
