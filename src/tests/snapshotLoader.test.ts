import { describe, expect, test } from "bun:test";
import type { RuntimeSnapshot } from "../src/features/relay/api/types";
import { loadRuntimeSnapshot } from "../src/features/relay/state/snapshotLoader";

describe("runtime snapshot loader", () => {
  test("Hosted does not contact a runtime", async () => {
    let calls = 0;
    expect(await loadRuntimeSnapshot("hosted", {
      localState: async () => { calls += 1; throw new Error("unexpected local access"); },
    })).toBeNull();
    expect(calls).toBe(0);
  });

  test("loads the local snapshot", async () => {
    const snapshot = { schemaVersion: 1 } as RuntimeSnapshot;
    expect(await loadRuntimeSnapshot("local", { localState: async () => snapshot })).toBe(snapshot);
  });

  test("does not hide local load failures", async () => {
    await expect(loadRuntimeSnapshot("local", {
      localState: async () => { throw new Error("unavailable"); },
    })).rejects.toThrow("unavailable");
  });
});
