import type { Analytics } from "./overviewAnalyticsModel";
import type { UsageTotals } from "../../api/types";
import { emptyUsageTotals } from "../../usageTotals";
import { createAnalyticsPersistence } from "./analyticsPersistence";

const MAX_CACHED_ANALYTICS = 8;
const ANALYTICS_REVALIDATION_COOLDOWN_MS = 1_000;
const ANALYTICS_STORAGE_KEY = "relay.overviewAnalyticsCache.v1";
type AnalyticsCacheEntry = { analytics: Analytics; updatedAt: number };
const analyticsCache = new Map<string, AnalyticsCacheEntry>();
const analyticsInFlight = new Map<string, Promise<Analytics | null>>();
let cacheHydrated = false;
let cacheGeneration = 0;
const persistence = createAnalyticsPersistence(persistCache, (work) => {
  const timer = globalThis.setTimeout(work, 250);
  return () => globalThis.clearTimeout(timer);
});

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", persistence.flush);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") persistence.flush();
  });
}

function validCounter(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function validTotals(value: unknown): value is UsageTotals {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const counters = value as Record<string, unknown>;
  const { apiEquivalent, ...fields } = emptyUsageTotals();
  const equivalent = counters["apiEquivalent"];
  return Object.keys(fields).every((key) => validCounter(counters[key])
    || (counters[key] === undefined && (key === "cacheWriteInputTokens" || key === "cacheWriteInputSamples")))
    && equivalent != null && typeof equivalent === "object" && !Array.isArray(equivalent)
    && Object.keys(apiEquivalent).every((key) => validCounter((equivalent as Record<string, unknown>)[key]));
}

function hydrateCache() {
  if (cacheHydrated) return;
  cacheHydrated = true;
  try {
    const stored = localStorage.getItem(ANALYTICS_STORAGE_KEY);
    if (!stored) return;
    const storedEntries: unknown = JSON.parse(stored);
    if (!Array.isArray(storedEntries)) return;
    storedEntries.slice(-MAX_CACHED_ANALYTICS).forEach((storedEntry) => {
      if (!storedEntry || typeof storedEntry !== "object") return;
      const storedEntryRecord = storedEntry as { scope?: unknown; value?: unknown };
      if (typeof storedEntryRecord.scope !== "string" || !storedEntryRecord.value || typeof storedEntryRecord.value !== "object") return;
      const analytics = storedEntryRecord.value as Partial<Analytics>;
      if (!validTotals(analytics.totals) || !Array.isArray(analytics.buckets)
        || !analytics.buckets.every((bucket) => bucket != null && Number.isFinite(bucket.startMs) && validTotals(bucket.totals))) return;
      // Display persisted snapshots immediately and revalidate after each restart.
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
  // Publish the in-memory report now; serialize only the latest bounded
  // cache later, or flush it when the document is hidden/closed.
  persistence.request();
}

export function clearOverviewAnalytics() {
  cacheGeneration += 1;
  cacheHydrated = true;
  analyticsCache.clear();
  analyticsInFlight.clear();
  persistence.request();
  persistence.flush();
}

export function isOverviewAnalyticsFresh(scope: string, now = Date.now()) {
  hydrateCache();
  const cacheEntry = analyticsCache.get(scope);
  const age = cacheEntry ? now - cacheEntry.updatedAt : Infinity;
  return age >= 0 && age < ANALYTICS_REVALIDATION_COOLDOWN_MS;
}

/** Deduplicates concurrent refreshes for the same report scope. */
export function loadOverviewAnalytics(scope: string, loader: () => Promise<Analytics | null>, force = false, signal?: AbortSignal): Promise<Analytics | null> {
  if (signal?.aborted) return Promise.reject(signal.reason);
  const existing = analyticsInFlight.get(scope);
  if (existing) {
    if (!force) return existing;
    // An event may queue revalidation behind a slow read. Navigation cancels
    // that intent, not the shared native read another visitor can still use.
    const reload = () => loadOverviewAnalytics(scope, loader, false, signal);
    return existing.then(reload, reload);
  }
  const generation = cacheGeneration;
  const analyticsRequest = loader()
    .then((loadedAnalytics) => {
      if (generation !== cacheGeneration) return null;
      if (loadedAnalytics) rememberOverviewAnalytics(scope, loadedAnalytics);
      return loadedAnalytics;
    })
    .finally(() => {
      if (analyticsInFlight.get(scope) === analyticsRequest) analyticsInFlight.delete(scope);
    });
  analyticsInFlight.set(scope, analyticsRequest);
  return analyticsRequest;
}
