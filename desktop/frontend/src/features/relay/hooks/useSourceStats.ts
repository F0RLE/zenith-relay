import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { RelayMode, SourceStats, SourceSummary } from "../api/types";
import { projectedSourceStats, settledSourceStats, type SourceStatsState } from "../sourceStatsModel";

type ScopedSourceStats = { scope: string; state: SourceStatsState };

function sourceStatsScope(mode: RelayMode, source: SourceSummary): string {
  return JSON.stringify([mode, source.baseUrl, source.secretAvailable, source.refreshRevision]);
}

/** One UI reader for cached observations. Background snapshots never initiate
 * another provider read; explicit refresh intent is confined to one call. */
export function useSourceStats(mode: RelayMode, sources: readonly SourceSummary[]) {
  const sourcesRef = useRef(sources);
  sourcesRef.current = sources;
  const sourceScopes = useMemo(() => Object.fromEntries(sources.map((source) => [source.id, sourceStatsScope(mode, source)])), [mode, sources]);
  const scopesRef = useRef(sourceScopes);
  scopesRef.current = sourceScopes;
  const scope = useMemo(() => JSON.stringify(Object.entries(sourceScopes).sort()), [sourceScopes]);
  const observations = useMemo(() => JSON.stringify(sources.map((source) => [source.id, source.providerStats]).sort()), [sources]);
  const [snapshot, setSnapshot] = useState<Record<string, ScopedSourceStats>>({});
  const activeScopes = useRef<Record<string, string>>({});
  const requests = useRef(new Map<string, symbol>());

  const refresh = useCallback(async (sourceId: string, force = false) => {
    const source = sourcesRef.current.find((candidate) => candidate.id === sourceId);
    if (!source?.secretAvailable) return;
    const requestedScope = sourceStatsScope(mode, source);
    if (activeScopes.current[sourceId] !== requestedScope) return;
    const request = Symbol();
    requests.current.set(sourceId, request);
    const isCurrentRequest = () => activeScopes.current[sourceId] === requestedScope
      && scopesRef.current[sourceId] === requestedScope && requests.current.get(sourceId) === request;
    const update = (apply: (previous: SourceStatsState) => SourceStatsState) => {
      if (!isCurrentRequest()) return;
      setSnapshot((previous) => {
        if (!isCurrentRequest()) return previous;
        const entry = previous[sourceId];
        const state = entry?.scope === requestedScope ? entry.state
          : source.providerStats ? settledSourceStats(null, source.providerStats) : { stats: null, loading: false, failed: false };
        return { ...previous, [sourceId]: { scope: requestedScope, state: apply(state) } };
      });
    };
    update((previous) => ({ ...previous, loading: true }));
    let sourceStats: SourceStats;
    try {
      sourceStats = await relayCommands.localSourceStats(sourceId, force);
    } catch (error) {
      update((previous) => ({ ...previous, loading: false, failed: true, error: "unavailable" }));
      throw error;
    }
    update((previous) => settledSourceStats(previous.stats, sourceStats));
    if (settledSourceStats(null, sourceStats).failed) {
      throw Object.assign(new Error("Source balance could not be refreshed"), { code: "upstream_error" });
    }
  }, [mode]);

  useEffect(() => () => { activeScopes.current = {}; requests.current.clear(); }, []);

  useEffect(() => {
    const currentSources = sourcesRef.current;
    const nextScopes = scopesRef.current;
    const previousScopes = activeScopes.current;
    activeScopes.current = nextScopes;
    for (const sourceId of requests.current.keys()) {
      if (previousScopes[sourceId] !== nextScopes[sourceId]) requests.current.delete(sourceId);
    }
    setSnapshot((previous) => {
      const next: Record<string, ScopedSourceStats> = {};
      for (const source of currentSources) {
        const sourceScope = nextScopes[source.id]!;
        const entry = previous[source.id];
        const previousState = entry?.scope === sourceScope ? entry.state : undefined;
        next[source.id] = {
          scope: sourceScope,
          state: source.providerStats ? projectedSourceStats(previousState, source.providerStats)
            : previousState ?? { stats: null, loading: false, failed: false },
        };
      }
      return next;
    });
    for (const source of currentSources) {
      if (previousScopes[source.id] !== nextScopes[source.id] && source.secretAvailable && !source.providerStats) {
        void refresh(source.id).catch(() => undefined);
      }
    }
  }, [scope, observations, refresh]);

  const stats = useMemo(() => Object.fromEntries(Object.entries(snapshot)
    .filter(([sourceId, entry]) => sourceScopes[sourceId] === entry.scope)
    .map(([sourceId, entry]) => [sourceId, entry.state])), [snapshot, sourceScopes]);
  return { stats, refresh };
}
