import { useCallback, useMemo } from "react";
import { relayCommands } from "../api/commands";
import type { PerformOptions } from "../state/relayOperationModel";
import type { RelayMode, SourceSummary } from "../api/types";
import type { SourceCatalogRefresh } from "../api/types/source";
import { refreshSourceCatalogs, type SourceRefreshExecutor } from "./sourceRefresh";

type Perform = (operationId: string, work: () => Promise<unknown>, successKey?: string, options?: PerformOptions) => Promise<boolean>;

type UseSourceRefreshInput = {
  mode: RelayMode;
  sources: ReadonlyArray<SourceSummary>;
  perform: Perform;
  refreshStats: (sourceId: string, force?: boolean) => Promise<void>;
};

/** Bridges the source-refresh policy to the page operation/feedback model. */
export function useSourceRefresh({ mode, sources, perform, refreshStats }: UseSourceRefreshInput) {
  const executor = useMemo<SourceRefreshExecutor>(() => ({
    testLocal: (sourceId) => completeSourceRefresh(
      relayCommands.refreshSourceData(sourceId, true),
      refreshStats(sourceId, true),
    ),
  }), [refreshStats]);

  const refresh = useCallback((sourceIds?: readonly string[], operationId = "sources-refresh-all") => {
    const sourceSnapshot = sources
      .filter((source) => !sourceIds || sourceIds.includes(source.id))
      .map(({ id: sourceId, secretAvailable }) => ({ id: sourceId, secretAvailable }));
    void perform(operationId, () => refreshSourceCatalogs({
      mode,
      sources: sourceSnapshot,
      executor,
    }), undefined, { backgroundRefresh: true });
  }, [mode, perform, sources, executor]);

  const refreshOne = useCallback((sourceId: string) => {
    refresh([sourceId], `source-refresh-${sourceId}`);
  }, [refresh]);

  return { refresh, refreshOne };
}

async function completeSourceRefresh(models: Promise<SourceCatalogRefresh>, stats: Promise<void>) {
  const [modelResult, statsResult] = await Promise.allSettled([models, stats]);
  if (modelResult.status === "rejected") throw modelResult.reason;
  return { ...modelResult.value, statsFailed: statsResult.status === "rejected" };
}
