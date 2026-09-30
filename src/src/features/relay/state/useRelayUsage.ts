import { useCallback, useRef, useState } from "react";
import type {
  LocalUsagePage,
  RelayMode,
  RemoteUsage,
  RemoteUsagePage,
  RemoteUsageQuery,
} from "../api/types";
import { LatestRequestGate } from "./latestRequestGate";

export type RelayUsageCommands = {
  localUsagePage: (query: RemoteUsageQuery) => Promise<LocalUsagePage>;
  remoteUsage: (query: RemoteUsageQuery) => Promise<RemoteUsagePage | null>;
};

export type UsageLoadOptions = {
  force?: boolean;
};

// A request can produce both state and usage events. Keep the last complete
// projection visible and avoid re-running the aggregate query for every event
// in the same short burst.
const USAGE_REVALIDATION_COOLDOWN_MS = 1_000;

type UsageCacheEntry<T> = { value: T; updatedAt: number };

function rememberUsage<T>(cache: Map<string, UsageCacheEntry<T>>, key: string, value: T) {
  cache.delete(key);
  cache.set(key, { value, updatedAt: Date.now() });
  while (cache.size > 8) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

/** Own paginated usage results and stale-request protection for both runtimes. */
export function useRelayUsage(commands: RelayUsageCommands) {
  const [localUsagePage, setLocalUsagePage] = useState<LocalUsagePage | null>(null);
  const [remoteUsage, setRemoteUsage] = useState<RemoteUsage[]>([]);
  const [remoteUsagePage, setRemoteUsagePage] = useState<RemoteUsagePage | null>(null);
  const localRequest = useRef(new LatestRequestGate());
  const remoteRequest = useRef(new LatestRequestGate());
  // Usage pages are expensive aggregates. Keep a small per-query cache so
  // navigating away and back can render the last complete result immediately
  // while the newest snapshot is refreshed in the background.
  const localCache = useRef(new Map<string, UsageCacheEntry<LocalUsagePage>>());
  const remoteCache = useRef(new Map<string, UsageCacheEntry<RemoteUsagePage | null>>());
  const localInFlight = useRef(new Map<string, Promise<LocalUsagePage>>());
  const remoteInFlight = useRef(new Map<string, Promise<RemoteUsagePage | null>>());
  const displayedLocalQueryKey = useRef<string | null>(null);
  const displayedRemoteQueryKey = useRef<string | null>(null);

  const loadLocalUsage = useCallback((query: RemoteUsageQuery, options: UsageLoadOptions = {}) => {
    const key = JSON.stringify(query);
    const queryChanged = displayedLocalQueryKey.current !== key;
    if (queryChanged) {
      // A cached report can become visible without starting a new request.
      // Invalidate the previous request anyway, otherwise its late result can
      // overwrite this query after the user has changed filters.
      localRequest.current.invalidate();
      displayedLocalQueryKey.current = key;
    }
    const cached = localCache.current.get(key);
    if (cached) {
      setLocalUsagePage(cached.value);
      if (!options.force && Date.now() - cached.updatedAt < USAGE_REVALIDATION_COOLDOWN_MS) {
        return Promise.resolve(cached.value);
      }
    } else if (queryChanged) {
      // A new filter must not show the previous report while it is loading.
      setLocalUsagePage(null);
    }
    const existing = localInFlight.current.get(key);
    if (existing) {
      // Re-adopt a deduplicated query after another filter was selected. Its
      // original completion was intentionally invalidated above, so this
      // current gate owns the visible result instead.
      return localRequest.current.run(() => existing, (value) => {
        rememberUsage(localCache.current, key, value);
        displayedLocalQueryKey.current = key;
        setLocalUsagePage(value);
      });
    }
    const request = localRequest.current.run(
      () => commands.localUsagePage(query),
      (value) => {
        rememberUsage(localCache.current, key, value);
        displayedLocalQueryKey.current = key;
        setLocalUsagePage(value);
      },
    );
    localInFlight.current.set(key, request);
    void request.then(() => {
      if (localInFlight.current.get(key) === request) localInFlight.current.delete(key);
    }, () => {
      if (localInFlight.current.get(key) === request) localInFlight.current.delete(key);
    });
    return request;
  }, [commands]);

  const loadRemoteUsage = useCallback((query: RemoteUsageQuery, options: UsageLoadOptions = {}) => {
    const key = JSON.stringify(query);
    const queryChanged = displayedRemoteQueryKey.current !== key;
    if (queryChanged) {
      // See the local branch: a cached query change must also invalidate a
      // previous remote aggregate that is still in flight.
      remoteRequest.current.invalidate();
      displayedRemoteQueryKey.current = key;
    }
    const cached = remoteCache.current.get(key);
    if (cached !== undefined) {
      setRemoteUsage(cached.value?.events ?? []);
      setRemoteUsagePage(cached.value);
      if (!options.force && Date.now() - cached.updatedAt < USAGE_REVALIDATION_COOLDOWN_MS) {
        return Promise.resolve(cached.value);
      }
    } else if (queryChanged) {
      // A new filter must not show the previous report while it is loading.
      setRemoteUsage([]);
      setRemoteUsagePage(null);
    }
    const existing = remoteInFlight.current.get(key);
    if (existing) {
      return remoteRequest.current.run(() => existing, (usage) => {
        rememberUsage(remoteCache.current, key, usage);
        displayedRemoteQueryKey.current = key;
        setRemoteUsage(usage?.events ?? []);
        setRemoteUsagePage(usage);
      });
    }
    const request = remoteRequest.current.run(
      () => commands.remoteUsage(query),
      (usage) => {
        rememberUsage(remoteCache.current, key, usage);
        displayedRemoteQueryKey.current = key;
        setRemoteUsage(usage?.events ?? []);
        setRemoteUsagePage(usage);
      },
    );
    remoteInFlight.current.set(key, request);
    void request.then(() => {
      if (remoteInFlight.current.get(key) === request) remoteInFlight.current.delete(key);
    }, () => {
      if (remoteInFlight.current.get(key) === request) remoteInFlight.current.delete(key);
    });
    return request;
  }, [commands]);

  const resetUsage = useCallback(() => {
    localRequest.current.invalidate();
    remoteRequest.current.invalidate();
    localInFlight.current.clear();
    remoteInFlight.current.clear();
    localCache.current.clear();
    remoteCache.current.clear();
    displayedLocalQueryKey.current = null;
    displayedRemoteQueryKey.current = null;
    setLocalUsagePage(null);
    setRemoteUsage([]);
    setRemoteUsagePage(null);
  }, []);

  const clearInactiveUsage = useCallback((mode: RelayMode) => {
    if (mode === "local") {
      remoteRequest.current.invalidate();
      remoteCache.current.clear();
      remoteInFlight.current.clear();
      displayedRemoteQueryKey.current = null;
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      return;
    }
    localRequest.current.invalidate();
    setLocalUsagePage(null);
    localCache.current.clear();
    localInFlight.current.clear();
    displayedLocalQueryKey.current = null;
    if (mode === "zenith") {
      remoteRequest.current.invalidate();
      remoteCache.current.clear();
      remoteInFlight.current.clear();
      displayedRemoteQueryKey.current = null;
      setRemoteUsage([]);
      setRemoteUsagePage(null);
    }
  }, []);

  return {
    localUsagePage,
    remoteUsage,
    remoteUsagePage,
    loadLocalUsage,
    loadRemoteUsage,
    resetUsage,
    clearInactiveUsage,
  };
}
