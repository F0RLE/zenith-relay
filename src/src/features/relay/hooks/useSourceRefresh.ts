import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { PerformOptions } from "../state/relayOperationModel";
import type { RelayMode, SourceSummary } from "../api/types";
import { refreshSourceCatalog, refreshSourceCatalogs, type SourceRefreshExecutor, type SourceRefreshReport } from "./sourceRefresh";

type Perform = (operationId: string, work: () => Promise<unknown>, successKey?: string, options?: PerformOptions) => Promise<boolean>;

type UseSourceRefreshInput = {
  mode: RelayMode;
  sources: ReadonlyArray<SourceSummary>;
  resetKey: string;
  perform: Perform;
  refreshStats: (sourceId: string, force?: boolean) => Promise<void>;
};

/** Bridges the source-refresh policy to the page operation/feedback model. */
export function useSourceRefresh({ mode, sources, resetKey, perform, refreshStats }: UseSourceRefreshInput) {
  const [report, setReport] = useState<SourceRefreshReport | null>(null);
  const contextRevision = useRef(0);
  const executor = useMemo<SourceRefreshExecutor>(() => ({
    testLocal: (sourceId) => completeSourceRefresh(
      relayCommands.testSource(sourceId),
      refreshStats(sourceId, true),
    ),
  }), [refreshStats]);

  useEffect(() => {
    contextRevision.current += 1;
    setReport(null);
  }, [resetKey]);

  const refresh = useCallback((sourceIds?: readonly string[]) => {
    const revision = contextRevision.current;
    const sourceSnapshot = sources
      .filter((source) => !sourceIds || sourceIds.includes(source.id))
      .map(({ id: sourceId, secretAvailable }) => ({ id: sourceId, secretAvailable }));
    let nextReport: SourceRefreshReport | undefined;
    setReport(null);
    void perform("sources-refresh-all", async () => {
      nextReport = await refreshSourceCatalogs({
        mode,
        sources: sourceSnapshot,
        executor,
      });
    }, undefined, { backgroundRefresh: true }).then((completed) => {
      if (completed && nextReport && revision === contextRevision.current) setReport(nextReport);
    });
  }, [mode, perform, sources, executor]);

  const refreshOne = useCallback((sourceId: string) => {
    void perform(
      `source-refresh-${sourceId}`,
      () => refreshSourceCatalog(mode, sourceId, executor),
      "feedback.refreshed",
      { backgroundRefresh: true },
    );
  }, [mode, perform, executor]);

  return { report, refresh, refreshOne };
}

async function completeSourceRefresh(models: Promise<unknown>, stats: Promise<void>) {
  const [modelResult, statsResult] = await Promise.allSettled([models, stats]);
  if (modelResult.status === "rejected") throw modelResult.reason;
  if (statsResult.status === "rejected") throw statsResult.reason;
  return modelResult.value;
}
