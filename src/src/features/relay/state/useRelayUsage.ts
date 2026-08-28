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

/** Own paginated usage results and stale-request protection for both runtimes. */
export function useRelayUsage(commands: RelayUsageCommands) {
  const [localUsagePage, setLocalUsagePage] = useState<LocalUsagePage | null>(null);
  const [remoteUsage, setRemoteUsage] = useState<RemoteUsage[]>([]);
  const [remoteUsagePage, setRemoteUsagePage] = useState<RemoteUsagePage | null>(null);
  const localRequest = useRef(new LatestRequestGate());
  const remoteRequest = useRef(new LatestRequestGate());

  const loadLocalUsage = useCallback((query: RemoteUsageQuery) => (
    localRequest.current.run(
      () => commands.localUsagePage(query),
      setLocalUsagePage,
    )
  ), [commands]);

  const loadRemoteUsage = useCallback((query: RemoteUsageQuery) => (
    remoteRequest.current.run(
      () => commands.remoteUsage(query),
      (usage) => {
        setRemoteUsage(usage?.events ?? []);
        setRemoteUsagePage(usage);
      },
    )
  ), [commands]);

  const resetUsage = useCallback(() => {
    localRequest.current.invalidate();
    remoteRequest.current.invalidate();
    setLocalUsagePage(null);
    setRemoteUsage([]);
    setRemoteUsagePage(null);
  }, []);

  const clearInactiveUsage = useCallback((mode: RelayMode) => {
    if (mode === "local") {
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      return;
    }
    setLocalUsagePage(null);
    if (mode === "zenith") {
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
