import { useCallback, useRef, useState } from "react";
import type { LocalUsagePage, UsageQuery } from "../api/types";
import { LatestRequestGate } from "./latestRequestGate";
import { shareUsageFilterChoices } from "./usagePageSharing";

export type RelayUsageCommands = {
  localUsagePage: (query: UsageQuery) => Promise<LocalUsagePage>;
};

export type UsageLoadOptions = { force?: boolean };

const USAGE_REVALIDATION_COOLDOWN_MS = 1_000;
type UsageCacheEntry<T> = { data: T; updatedAt: number };

function rememberUsage<T>(cache: Map<string, UsageCacheEntry<T>>, key: string, data: T) {
  cache.delete(key);
  cache.set(key, { data, updatedAt: Date.now() });
  while (cache.size > 8) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

/** Keeps the last complete local report without allowing stale filters to win. */
export function useRelayUsage(commands: RelayUsageCommands) {
  const [localUsagePage, setLocalUsagePage] = useState<LocalUsagePage | null>(null);
  const request = useRef(new LatestRequestGate());
  const cache = useRef(new Map<string, UsageCacheEntry<LocalUsagePage>>());
  const inFlight = useRef(new Map<string, Promise<LocalUsagePage>>());
  const displayedQueryKey = useRef<string | null>(null);
  const filterChoices = useRef<LocalUsagePage["filterOptions"]>(undefined);

  const loadLocalUsage = useCallback((query: UsageQuery, options: UsageLoadOptions = {}) => {
    const key = JSON.stringify(query);
    const queryChanged = displayedQueryKey.current !== key;
    if (queryChanged) {
      // Invalidate even when a cached report avoids starting another request.
      request.current.invalidate();
      displayedQueryKey.current = key;
    }
    const cached = cache.current.get(key);
    if (cached) {
      filterChoices.current = cached.data.filterOptions;
      setLocalUsagePage(cached.data);
      const age = Date.now() - cached.updatedAt;
      if (!options.force && age >= 0 && age < USAGE_REVALIDATION_COOLDOWN_MS) {
        return Promise.resolve(cached.data);
      }
    } else if (queryChanged) {
      setLocalUsagePage(null);
    }
    const accept = (page: LocalUsagePage) => {
      const shared = shareUsageFilterChoices(page, filterChoices.current);
      rememberUsage(cache.current, key, shared);
      displayedQueryKey.current = key;
      filterChoices.current = shared.filterOptions;
      setLocalUsagePage(shared);
    };
    const existing = inFlight.current.get(key);
    // Re-adopt an existing query under the current filter's request gate.
    if (existing) return request.current.run(() => existing, accept);
    const loading = request.current.run(() => commands.localUsagePage(query), accept);
    inFlight.current.set(key, loading);
    const settled = () => {
      if (inFlight.current.get(key) === loading) inFlight.current.delete(key);
    };
    void loading.then(settled, settled);
    return loading;
  }, [commands]);

  const resetUsage = useCallback(() => {
    request.current.invalidate();
    inFlight.current.clear();
    cache.current.clear();
    displayedQueryKey.current = null;
    filterChoices.current = undefined;
    setLocalUsagePage(null);
  }, []);

  return { localUsagePage, loadLocalUsage, resetUsage };
}
