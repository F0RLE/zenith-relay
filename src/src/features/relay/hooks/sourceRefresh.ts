import type { RelayMode, SourceSummary } from "../api/types";
import type { SourceCatalogRefresh } from "../api/types/source";

export type SourceRefreshReport = {
  refreshed: number;
  failed: number;
  skipped: number;
  addedModels: number;
  removedModels: number;
};

export type SourceRefreshResult = SourceCatalogRefresh & { statsFailed?: boolean };

export type SourceRefreshExecutor = {
  testLocal: (sourceId: string) => Promise<SourceRefreshResult>;
};

export type SourceRefreshInput = {
  mode: RelayMode;
  sources: ReadonlyArray<Pick<SourceSummary, "id" | "secretAvailable">>;
  executor: SourceRefreshExecutor;
};

export function requireCompleteSourceCatalog(result: SourceCatalogRefresh) {
  if (result.status === "failed" || result.status === "partial") {
    throw Object.assign(new Error("Source model catalog could not be fully refreshed"), { code: "upstream_error" });
  }
}

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
  let addedModels = 0;
  let removedModels = 0;

  // Discovery rebuilds shared runtime metadata; serialize it so a slower
  // source cannot overwrite the catalog produced by a later source.
  for (const source of sources) {
    if (!source.secretAvailable) {
      skipped += 1;
      continue;
    }
    try {
      const result = await refreshSourceCatalog(mode, source.id, executor);
      addedModels += result.addedModels;
      removedModels += result.removedModels;
      if (result.status === "manual") skipped += 1;
      else if (result.status !== "complete" || result.statsFailed) failed += 1;
      else refreshed += 1;
    } catch {
      failed += 1;
    }
  }

  return { refreshed, failed, skipped, addedModels, removedModels };
}
