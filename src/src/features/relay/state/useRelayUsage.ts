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
    const cached = localCache.current.get(key);
    if (cached) {
      displayedLocalQueryKey.current = key;
      setLocalUsagePage(cached.value);
      if (!options.force && Date.now() - cached.updatedAt < USAGE_REVALIDATION_COOLDOWN_MS) {
        return Promise.resolve(cached.value);
      }
    } else if (displayedLocalQueryKey.current !== key) {
      // A new filter must not show the previous report while it is loading.
      displayedLocalQueryKey.current = key;
      setLocalUsagePage(null);
    }
    const existing = localInFlight.current.get(key);
    if (existing) return existing;
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
    const cached = remoteCache.current.get(key);
    if (cached !== undefined) {
      displayedRemoteQueryKey.current = key;
      setRemoteUsage(cached.value?.events ?? []);
      setRemoteUsagePage(cached.value);
      if (!options.force && Date.now() - cached.updatedAt < USAGE_REVALIDATION_COOLDOWN_MS) {
        return Promise.resolve(cached.value);
      }
    } else if (displayedRemoteQueryKey.current !== key) {
      // A new filter must not show the previous report while it is loading.
      displayedRemoteQueryKey.current = key;
      setRemoteUsage([]);
      setRemoteUsagePage(null);
    }
    const existing = remoteInFlight.current.get(key);
    if (existing) return existing;
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
      remoteCache.current.clear();
      remoteInFlight.current.clear();
      displayedRemoteQueryKey.current = null;
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      return;
    }
    setLocalUsagePage(null);
    localCache.current.clear();
    localInFlight.current.clear();
    displayedLocalQueryKey.current = null;
    if (mode === "zenith") {
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
