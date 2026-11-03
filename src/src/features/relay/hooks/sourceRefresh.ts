import type { RelayMode, SourceSummary } from "../api/types";

export type SourceRefreshReport = {
  refreshed: number;
  failed: number;
  skipped: number;
};

export type SourceRefreshExecutor = {
  testLocal: (sourceId: string) => Promise<unknown>;
};

export type SourceRefreshInput = {
  mode: RelayMode;
  sources: ReadonlyArray<Pick<SourceSummary, "id" | "secretAvailable">>;
  executor: SourceRefreshExecutor;
};

export async function refreshSourceCatalog(
  mode: RelayMode,
  sourceId: string,
  executor: SourceRefreshExecutor,
) {
  if (mode !== "local") throw new Error("Catalog refresh requires the local pool");
  return executor.testLocal(sourceId);
}

/** Refresh source catalogs in a deterministic order and classify each result. */
export async function refreshSourceCatalogs({ mode, sources, executor }: SourceRefreshInput): Promise<SourceRefreshReport> {
  let refreshed = 0;
  let failed = 0;
  let skipped = 0;

  // Discovery rebuilds shared runtime metadata; serialize it so a slower
  // source cannot overwrite the catalog produced by a later source.
  for (const source of sources) {
    if (!source.secretAvailable) {
      skipped += 1;
      continue;
    }
    try {
      await refreshSourceCatalog(mode, source.id, executor);
      refreshed += 1;
    } catch {
      failed += 1;
    }
  }

  return { refreshed, failed, skipped };
}
