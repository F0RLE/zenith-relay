import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { relayCommands } from "../../api/commands";
import type { LocalUsagePage, UsageQuery } from "../../api/types";

/** History-wide choices are read only when a filter is actually opened. */
export function useLazyUsageFilterOptions(query: UsageQuery, revision: number, supplied: LocalUsagePage["filterOptions"], onError: () => void) {
  const period = useMemo<UsageQuery>(() => ({
    ...(query.range ? { range: query.range } : {}),
    ...(query.fromMs !== undefined ? { fromMs: query.fromMs } : {}),
    ...(query.toMs !== undefined ? { toMs: query.toMs } : {}),
    includeReport: false,
    includeEvents: false,
    includeModels: false,
    includePoolMembers: false,
    includeFilterOptions: true,
  }), [query.range, query.fromMs, query.toMs]);
  const key = JSON.stringify(period);
  const [loaded, setLoaded] = useState<{ key: string; revision: number; options: LocalUsagePage["filterOptions"] } | null>(null);
  const pending = useRef(new Set<string>());
  const currentScope = useRef({ key, revision });
  currentScope.current = { key, revision };
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const load = useCallback(() => {
    if (supplied || loaded?.key === key && loaded.revision === revision) return;
    const requestKey = `${key}:${revision}`;
    if (pending.current.has(requestKey)) return;
    pending.current.add(requestKey);
    const isCurrent = () => mounted.current && currentScope.current.key === key && currentScope.current.revision === revision;
    void relayCommands.localUsagePage(period).then((page) => {
      // An older history/period read must not replace newer choices or make
      // the next menu opening unnecessarily request the same choices again.
      if (isCurrent()) setLoaded({ key, revision, options: page.filterOptions });
    }).catch(() => { if (isCurrent()) onError(); }).finally(() => { pending.current.delete(requestKey); });
  }, [key, loaded, onError, period, revision, supplied]);
  return { options: supplied ?? (loaded?.key === key ? loaded.options : undefined), load };
}
