import type { Analytics } from "./overviewAnalyticsModel";

const MAX_CACHED_ANALYTICS = 8;
const ANALYTICS_REVALIDATION_COOLDOWN_MS = 1_000;
const ANALYTICS_STORAGE_KEY = "relay.overviewAnalyticsCache.v1";
type AnalyticsCacheEntry = { value: Analytics; updatedAt: number };
const analyticsCache = new Map<string, AnalyticsCacheEntry>();
const analyticsInFlight = new Map<string, Promise<Analytics | null>>();
let cacheHydrated = false;

function hydrateCache() {
  if (cacheHydrated) return;
  cacheHydrated = true;
  try {
    const stored = localStorage.getItem(ANALYTICS_STORAGE_KEY);
    if (!stored) return;
    const entries: unknown = JSON.parse(stored);
    if (!Array.isArray(entries)) return;
    entries.forEach((entry) => {
      if (!entry || typeof entry !== "object") return;
      const record = entry as { scope?: unknown; value?: unknown };
      if (typeof record.scope !== "string" || !record.value || typeof record.value !== "object") return;
      const value = record.value as Partial<Analytics>;
      if (!value.totals || typeof value.totals !== "object" || !Array.isArray(value.buckets)) return;
      // Persisted snapshots are for immediate display only. Always revalidate
      // them after a process restart instead of treating them as fresh.
      analyticsCache.set(record.scope, { value: value as Analytics, updatedAt: 0 });
    });
  } catch {
    // A corrupt or unavailable browser store must not block the Overview.
  }
}

function persistCache() {
  try {
    localStorage.setItem(ANALYTICS_STORAGE_KEY, JSON.stringify(
      Array.from(analyticsCache, ([scope, entry]) => ({ scope, value: entry.value })),
    ));
  } catch {
    // Storage quota and privacy-mode failures are non-fatal for analytics.
  }
}

export function getCachedOverviewAnalytics(scope: string) {
  hydrateCache();
  return analyticsCache.get(scope)?.value ?? null;
}

export function rememberOverviewAnalytics(scope: string, analytics: Analytics) {
  hydrateCache();
  analyticsCache.delete(scope);
  analyticsCache.set(scope, { value: analytics, updatedAt: Date.now() });
  while (analyticsCache.size > MAX_CACHED_ANALYTICS) {
    const oldestScope = analyticsCache.keys().next().value;
    if (oldestScope === undefined) break;
    analyticsCache.delete(oldestScope);
  }
  persistCache();
}

export function isOverviewAnalyticsFresh(scope: string, now = Date.now()) {
  hydrateCache();
  const entry = analyticsCache.get(scope);
  return Boolean(entry && now - entry.updatedAt < ANALYTICS_REVALIDATION_COOLDOWN_MS);
}

/** Deduplicates concurrent refreshes for the same report scope. */
export function loadOverviewAnalytics(scope: string, loader: () => Promise<Analytics | null>) {
  const existing = analyticsInFlight.get(scope);
  if (existing) return existing;
  const request = loader()
    .then((value) => {
      if (value) rememberOverviewAnalytics(scope, value);
      return value;
    })
    .finally(() => {
      if (analyticsInFlight.get(scope) === request) analyticsInFlight.delete(scope);
    });
  analyticsInFlight.set(scope, request);
  return request;
}
