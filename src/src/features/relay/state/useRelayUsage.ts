import { useCallback, useRef, useState } from "react";
import type { LocalUsagePage, UsageQuery } from "../api/types";
import { LatestRequestGate } from "./latestRequestGate";
import { shareUsageFilterChoices } from "./usagePageSharing";
import { rememberUsagePreview, usagePreview } from "./usagePreview";

export type RelayUsageCommands = {
  localUsagePage: (query: UsageQuery) => Promise<LocalUsagePage>;
};

export type UsageLoadOptions = { force?: boolean; revision?: number; signal?: AbortSignal };

const USAGE_REVALIDATION_COOLDOWN_MS = 1_000;
type UsageCacheEntry = { key: string; data: LocalUsagePage; updatedAt: number; revision: number | undefined };
type PendingUsageRead = {
  key: string;
  epoch: number;
  revision: number | undefined;
  promise: Promise<LocalUsagePage>;
};

function legacyPreviewKey(query: UsageQuery) {
  if (query.includeReport === undefined) return null;
  // Reuse a same-scope preview saved before the lightweight IPC hint existed.
  // It is display-only and still revalidated by the current native request.
  const { includeReport: _includeReport, ...legacy } = query;
  return JSON.stringify({ ...legacy, includeFilterOptions: true });
}

function previewFor(query: UsageQuery) {
  const current = usagePreview.peek(JSON.stringify(query));
  const legacyKey = legacyPreviewKey(query);
  return current ?? (legacyKey ? usagePreview.peek(legacyKey) : null);
}

async function restorePreviewFor(query: UsageQuery) {
  const key = JSON.stringify(query);
  await usagePreview.restoreKey(key);
  const legacyKey = legacyPreviewKey(query);
  if (!usagePreview.peek(key) && legacyKey) await usagePreview.restoreKey(legacyKey);
}

/** Keeps the last complete local report without allowing stale filters to win. */
export function useRelayUsage(commands: RelayUsageCommands) {
  const [report, setReport] = useState<{ key: string; data: LocalUsagePage } | null>(null);
  const request = useRef(new LatestRequestGate());
  const cache = useRef<UsageCacheEntry | null>(null);
  const displayedQueryKey = useRef<string | null>(null);
  const filterChoices = useRef<LocalUsagePage["filterOptions"]>(undefined);
  const resetRevision = useRef(0);
  const pendingRead = useRef<PendingUsageRead | null>(null);

  // Read during render, before the loading effect adopts the cached report.
  // Keep only the last selected query; other pages load on navigation.
  const getCachedLocalUsage = useCallback((query: UsageQuery) => {
    const key = JSON.stringify(query);
    return cache.current?.key === key ? cache.current.data : previewFor(query);
  }, []);

  const loadLocalUsage = useCallback(async (query: UsageQuery, options: UsageLoadOptions = {}) => {
    options.signal?.throwIfAborted();
    if (options.force) cache.current = null;
    const key = JSON.stringify(query);
    const requestedRevision = resetRevision.current;
    if (cache.current?.key !== key) {
      void restorePreviewFor(query).then(() => {
        if (options.signal?.aborted || resetRevision.current !== requestedRevision || displayedQueryKey.current !== key || cache.current?.key === key) return;
        const previous = previewFor(query);
        if (previous) {
          cache.current = { key, data: previous, updatedAt: 0, revision: undefined };
          setReport({ key, data: previous });
        }
      });
    }
    const queryChanged = displayedQueryKey.current !== key;
    if (queryChanged) {
      // Invalidate even when a cached report avoids starting another request.
      request.current.invalidate();
      displayedQueryKey.current = key;
    }
    const cached = cache.current?.key === key ? cache.current : null;
    if (cached) {
      filterChoices.current = cached.data.filterOptions;
      setReport((previous) => previous?.key === key && previous.data === cached.data ? previous : { key, data: cached.data });
      const age = Date.now() - cached.updatedAt;
      const unchangedHistory = options.revision !== undefined && cached.revision === options.revision
        && !query.range && query.fromMs === undefined && query.toMs === undefined;
      const sameRevision = options.revision === undefined || cached.revision === options.revision;
      if (!options.force && sameRevision && age >= 0 && (unchangedHistory || age < USAGE_REVALIDATION_COOLDOWN_MS)) {
        request.current.invalidate();
        return cached.data;
      }
    } else if (queryChanged) {
      setReport(null);
    }
    const accept = (page: LocalUsagePage) => {
      if (options.signal?.aborted) return;
      const shared = shareUsageFilterChoices(page, filterChoices.current);
      if (resetRevision.current === requestedRevision) {
        cache.current = { key, data: shared, updatedAt: Date.now(), revision: options.revision };
      }
      rememberUsagePreview(key, shared);
      displayedQueryKey.current = key;
      filterChoices.current = shared.filterOptions;
      setReport({ key, data: shared });
    };
    return request.current.run(async () => {
      // Native reads cannot be canceled. One owner survives page navigation;
      // obsolete waiters do no I/O and re-entry can share the current read.
      while (pendingRead.current) {
        const pending = pendingRead.current;
        if (pending.key === key && pending.epoch === requestedRevision && pending.revision === options.revision) {
          return pending.promise;
        }
        await pending.promise.catch(() => undefined);
        options.signal?.throwIfAborted();
      }
      const pending = { key, epoch: requestedRevision, revision: options.revision, promise: commands.localUsagePage(query) };
      pendingRead.current = pending;
      try {
        return await pending.promise;
      } finally {
        if (pendingRead.current === pending) pendingRead.current = null;
      }
    }, accept);
  }, [commands]);

  const invalidateUsageCache = useCallback(() => {
    resetRevision.current += 1;
    cache.current = null;
  }, []);

  const resetUsage = useCallback(() => {
    invalidateUsageCache();
    request.current.invalidate();
    displayedQueryKey.current = null;
    filterChoices.current = undefined;
    setReport(null);
  }, [invalidateUsageCache]);

  const invalidateLocalUsage = useCallback(async () => {
    resetUsage();
    await usagePreview.clear();
  }, [resetUsage]);

  return { localUsagePage: report?.data ?? null, localUsageQueryKey: report?.key ?? null, getCachedLocalUsage, loadLocalUsage, resetUsage, invalidateLocalUsage, invalidateUsageCache };
}
