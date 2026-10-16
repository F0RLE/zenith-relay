import type { Analytics } from "./overviewAnalyticsModel";

const MAX_CACHED_ANALYTICS = 8;
const ANALYTICS_REVALIDATION_COOLDOWN_MS = 1_000;
const ANALYTICS_STORAGE_KEY = "relay.overviewAnalyticsCache.v1";
type AnalyticsCacheEntry = { analytics: Analytics; updatedAt: number };
const analyticsCache = new Map<string, AnalyticsCacheEntry>();
const analyticsInFlight = new Map<string, Promise<Analytics | null>>();
let cacheHydrated = false;

function hydrateCache() {
  if (cacheHydrated) return;
  cacheHydrated = true;
  try {
    const stored = localStorage.getItem(ANALYTICS_STORAGE_KEY);
    if (!stored) return;
    const storedEntries: unknown = JSON.parse(stored);
    if (!Array.isArray(storedEntries)) return;
    storedEntries.forEach((storedEntry) => {
      if (!storedEntry || typeof storedEntry !== "object") return;
      const storedEntryRecord = storedEntry as { scope?: unknown; value?: unknown };
      if (typeof storedEntryRecord.scope !== "string" || !storedEntryRecord.value || typeof storedEntryRecord.value !== "object") return;
      const analytics = storedEntryRecord.value as Partial<Analytics>;
      if (!analytics.totals || typeof analytics.totals !== "object" || !Array.isArray(analytics.buckets)) return;
      // Persisted snapshots are for immediate display only. Always revalidate
      // them after a process restart instead of treating them as fresh.
      analyticsCache.set(storedEntryRecord.scope, { analytics: analytics as Analytics, updatedAt: 0 });
    });
  } catch {
    // A corrupt or unavailable browser store must not block the Overview.
  }
}

function persistCache() {
  try {
    localStorage.setItem(ANALYTICS_STORAGE_KEY, JSON.stringify(
      Array.from(analyticsCache, ([scope, cachedScope]) => ({
        scope,
        value: cachedScope.analytics,
      })),
    ));
  } catch {
    // Storage quota and privacy-mode failures are non-fatal for analytics.
  }
}

export function getCachedOverviewAnalytics(scope: string) {
  hydrateCache();
  return analyticsCache.get(scope)?.analytics ?? null;
}

export function rememberOverviewAnalytics(scope: string, analytics: Analytics) {
  hydrateCache();
  analyticsCache.delete(scope);
  analyticsCache.set(scope, { analytics, updatedAt: Date.now() });
  while (analyticsCache.size > MAX_CACHED_ANALYTICS) {
    const oldestScope = analyticsCache.keys().next().value;
    if (oldestScope === undefined) break;
    analyticsCache.delete(oldestScope);
  }
  persistCache();
}

export function isOverviewAnalyticsFresh(scope: string, now = Date.now()) {
  hydrateCache();
  const cacheEntry = analyticsCache.get(scope);
  return Boolean(cacheEntry && now - cacheEntry.updatedAt < ANALYTICS_REVALIDATION_COOLDOWN_MS);
}

/** Deduplicates concurrent refreshes for the same report scope. */
export function loadOverviewAnalytics(scope: string, loader: () => Promise<Analytics | null>) {
  const existing = analyticsInFlight.get(scope);
  if (existing) return existing;
  const analyticsRequest = loader()
    .then((loadedAnalytics) => {
      if (loadedAnalytics) rememberOverviewAnalytics(scope, loadedAnalytics);
      return loadedAnalytics;
    })
    .finally(() => {
      if (analyticsInFlight.get(scope) === analyticsRequest) analyticsInFlight.delete(scope);
    });
  analyticsInFlight.set(scope, analyticsRequest);
  return analyticsRequest;
}
