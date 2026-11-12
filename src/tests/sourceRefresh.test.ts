import { describe, expect, test } from "bun:test";
import { refreshSourceCatalog, refreshSourceCatalogs, requireCompleteSourceCatalog } from "../src/features/relay/hooks/sourceRefresh";

describe("source catalog refresh", () => {
  test("single-source pool actions reject incomplete refreshes and preserve manual lists", () => {
    for (const status of ["failed", "partial"] as const) {
      expect(() => requireCompleteSourceCatalog({ sourceId: "synthetic", status, addedModels: 0, removedModels: 0 })).toThrow("could not be fully refreshed");
    }
    for (const status of ["complete", "manual"] as const) {
      expect(() => requireCompleteSourceCatalog({ sourceId: "synthetic", status, addedModels: 0, removedModels: 0 })).not.toThrow();
    }
  });
  test("refreshes local sources sequentially and isolates individual failures", async () => {
    const events: string[] = [];
    const report = await refreshSourceCatalogs({
      mode: "local",
      sources: [
        { id: "first", secretAvailable: true },
        { id: "unavailable", secretAvailable: false },
        { id: "second", secretAvailable: true },
      ],
      executor: {
        testLocal: async (id) => {
          events.push(`start:${id}`);
          await Promise.resolve();
          events.push(`end:${id}`);
          if (id === "second") throw new Error("source unavailable");
          return { sourceId: id, status: "complete", addedModels: 2, removedModels: 1 };
        },
      },
    });

    expect(events).toEqual(["start:first", "end:first", "start:second", "end:second"]);
    expect(report).toEqual({ refreshed: 1, failed: 1, skipped: 1, addedModels: 2, removedModels: 1 });
  });

  test("Hosted cannot contact local source credentials", async () => {
    const calls: string[] = [];
    const report = await refreshSourceCatalogs({
      mode: "hosted",
      sources: [{ id: "local-source", secretAvailable: true }],
      executor: {
        testLocal: async (id) => {
          calls.push(`local:${id}`);
          return { sourceId: id, status: "complete", addedModels: 0, removedModels: 0 };
        },
      },
    });

    expect(calls).toEqual([]);
    expect(report).toEqual({ refreshed: 0, failed: 1, skipped: 0, addedModels: 0, removedModels: 0 });
  });

  test("delegates a single source through the same mode policy as bulk refresh", async () => {
    const calls: string[] = [];
    await refreshSourceCatalog("local", "source-1", {
      testLocal: async (id) => {
        calls.push(`local:${id}`);
        return { sourceId: id, status: "complete", addedModels: 0, removedModels: 0 };
      },
    });
    expect(calls).toEqual(["local:source-1"]);
  });

  test("retains committed model changes while reporting partial catalog or balance failures", async () => {
    const report = await refreshSourceCatalogs({
      mode: "local",
      sources: ["partial", "balance", "failed", "manual"].map((id) => ({ id, secretAvailable: true })),
      executor: {
        testLocal: async (id) => {
          if (id === "partial") return { sourceId: id, status: "partial", addedModels: 2, removedModels: 1 };
          if (id === "balance") return { sourceId: id, status: "complete", addedModels: 1, removedModels: 0, statsFailed: true };
          return { sourceId: id, status: id === "manual" ? "manual" : "failed", addedModels: 0, removedModels: 0 };
        },
      },
    });
    expect(report).toEqual({ refreshed: 0, failed: 3, skipped: 1, addedModels: 3, removedModels: 1 });
  });

  test("unchanged successful catalogs report zero model changes", async () => {
    const report = await refreshSourceCatalogs({
      mode: "local",
      sources: [{ id: "same", secretAvailable: true }],
      executor: { testLocal: async (sourceId) => ({ sourceId, status: "complete", addedModels: 0, removedModels: 0 }) },
    });
    expect(report).toEqual({ refreshed: 1, failed: 0, skipped: 0, addedModels: 0, removedModels: 0 });
  });
});
