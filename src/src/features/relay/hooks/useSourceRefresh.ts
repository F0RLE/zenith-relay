import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../api/commands";
import type { RelayMode, SourceSummary } from "../api/types";
import { refreshSourceCatalog, refreshSourceCatalogs, type SourceRefreshExecutor, type SourceRefreshReport } from "./sourceRefresh";

type Perform = (id: string, work: () => Promise<unknown>, successKey?: string) => Promise<boolean>;

type UseSourceRefreshInput = {
  mode: RelayMode;
  sources: ReadonlyArray<SourceSummary>;
  resetKey: string;
  perform: Perform;
};

const sourceRefreshExecutor: SourceRefreshExecutor = {
  testLocal: relayCommands.testSource,
  testRemote: (sourceId) => relayCommands.remoteAction({ type: "test_source", id: sourceId }),
};

/** Bridges the source-refresh policy to the page operation/feedback model. */
export function useSourceRefresh({ mode, sources, resetKey, perform }: UseSourceRefreshInput) {
  const [report, setReport] = useState<SourceRefreshReport | null>(null);
  const contextRevision = useRef(0);

  useEffect(() => {
    contextRevision.current += 1;
    setReport(null);
  }, [resetKey]);

  const refresh = useCallback(() => {
    const revision = contextRevision.current;
    const sourceSnapshot = sources.map(({ id, secretAvailable }) => ({ id, secretAvailable }));
    let nextReport: SourceRefreshReport | undefined;
    setReport(null);
    void perform("sources-refresh-all", async () => {
      nextReport = await refreshSourceCatalogs({
        mode,
        sources: sourceSnapshot,
        executor: sourceRefreshExecutor,
      });
    }).then((completed) => {
      if (completed && nextReport && revision === contextRevision.current) setReport(nextReport);
    });
  }, [mode, perform, sources]);

  const refreshOne = useCallback((sourceId: string) => {
    void perform(
      `source-refresh-${sourceId}`,
      () => refreshSourceCatalog(mode, sourceId, sourceRefreshExecutor),
      "feedback.refreshed",
    );
  }, [mode, perform]);

  return { report, refresh, refreshOne };
}
