import { describe, expect, test } from "bun:test";
import { refreshSourceCatalog, refreshSourceCatalogs } from "../src/features/relay/hooks/sourceRefresh";

describe("source catalog refresh", () => {
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
        },
        testRemote: async () => { throw new Error("remote executor should not run"); },
      },
    });

    expect(events).toEqual(["start:first", "end:first", "start:second", "end:second"]);
    expect(report).toEqual({ refreshed: 1, failed: 1, skipped: 1 });
  });

  test("uses the remote executor only for a remote runtime", async () => {
    const calls: string[] = [];
    const report = await refreshSourceCatalogs({
      mode: "remote",
      sources: [{ id: "remote-source", secretAvailable: true }],
      executor: {
        testLocal: async (id) => { calls.push(`local:${id}`); },
        testRemote: async (id) => { calls.push(`remote:${id}`); },
      },
    });

    expect(calls).toEqual(["remote:remote-source"]);
    expect(report).toEqual({ refreshed: 1, failed: 0, skipped: 0 });
  });

  test("delegates a single source through the same mode policy as bulk refresh", async () => {
    const calls: string[] = [];
    await refreshSourceCatalog("local", "source-1", {
      testLocal: async (id) => { calls.push(`local:${id}`); },
      testRemote: async (id) => { calls.push(`remote:${id}`); },
    });
    expect(calls).toEqual(["local:source-1"]);
  });
});
