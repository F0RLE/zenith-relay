import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { RelayMode, SourceSummary } from "../api/types";
import { projectedSourceStats, settledSourceStats, type SourceStatsState } from "../sourceStatsModel";

/** One UI reader for cached observations. Background snapshots never initiate
 * another provider read; explicit refresh intent is confined to one call. */
export function useSourceStats(mode: RelayMode, sources: readonly SourceSummary[]) {
  const sourcesRef = useRef(sources);
  sourcesRef.current = sources;
  const scope = JSON.stringify([mode, sources.map((source) => [source.id, source.baseUrl, source.secretAvailable, source.refreshRevision]).sort()]);
  const observations = JSON.stringify(sources.map((source) => [source.id, source.providerStats]).sort());
  const [snapshot, setSnapshot] = useState<{ scope: string; values: Record<string, SourceStatsState> }>({ scope, values: {} });
  const setStats = useCallback((update: (previous: Record<string, SourceStatsState>) => Record<string, SourceStatsState>) => {
    setSnapshot((previous) => ({ scope, values: update(previous.scope === scope ? previous.values : {}) }));
  }, [scope]);
  const activeScope = useRef<string | null>(null);
  const generation = useRef(0);
  const requests = useRef<Record<string, number>>({});

  const refresh = useCallback(async (sourceId: string, force = false) => {
    if (activeScope.current !== scope || !sourcesRef.current.some((source) => source.id === sourceId && source.secretAvailable)) return;
    const currentGeneration = generation.current;
    const request = (requests.current[sourceId] ?? 0) + 1;
    requests.current[sourceId] = request;
    const current = () => generation.current === currentGeneration && requests.current[sourceId] === request;
    setStats((previous) => ({ ...previous, [sourceId]: { value: previous[sourceId]?.value ?? null, loading: true, failed: false } }));
    try {
      const value = await (mode === "remote" ? relayCommands.remoteSourceStats(sourceId, force) : relayCommands.localSourceStats(sourceId, force));
      if (current()) setStats((previous) => ({ ...previous, [sourceId]: settledSourceStats(previous[sourceId]?.value ?? null, value) }));
    } catch {
      if (current()) setStats((previous) => ({ ...previous, [sourceId]: { value: previous[sourceId]?.value ?? null, loading: false, failed: true, error: "unavailable" } }));
    }
  }, [mode, scope, setStats]);

  useEffect(() => {
    generation.current += 1;
    activeScope.current = scope;
    requests.current = {};
    const initial: Record<string, SourceStatsState> = {};
    for (const source of sourcesRef.current) {
      if (source.providerStats) initial[source.id] = settledSourceStats(null, source.providerStats);
    }
    setStats(() => initial);
    for (const source of sourcesRef.current) {
      if (source.secretAvailable && !source.providerStats) void refresh(source.id);
    }
    return () => { generation.current += 1; activeScope.current = null; };
  }, [scope, refresh, setStats]);

  useEffect(() => {
    setStats((previous) => {
      const next = { ...previous };
      for (const source of sourcesRef.current) {
        if (source.providerStats) next[source.id] = projectedSourceStats(previous[source.id], source.providerStats);
      }
      return next;
    });
  }, [scope, observations, setStats]);

  return { stats: snapshot.scope === scope ? snapshot.values : {}, refresh };
}
