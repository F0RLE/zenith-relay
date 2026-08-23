import { describe, expect, test } from "bun:test";
import type { UiState } from "../src/features/relay/api/commands";
import type { RuntimeSnapshot } from "../src/features/relay/api/types";
import { loadRuntimeSnapshot, type RuntimeSnapshotCommands } from "../src/features/relay/state/snapshotLoader";

const localSnapshot = { runtimeTarget: { kind: "local" } } as RuntimeSnapshot;
const remoteSnapshot = { runtimeTarget: { kind: "remote" } } as RuntimeSnapshot;
const readyState = { providerActive: true, codexRunning: false, hasSavedApiKey: true } satisfies UiState;

function commands(overrides: Partial<RuntimeSnapshotCommands> = {}): RuntimeSnapshotCommands {
  return {
    localState: async () => localSnapshot,
    remoteState: async () => remoteSnapshot,
    readyState: async () => readyState,
    ...overrides,
  };
}

describe("runtime snapshot loader", () => {
  test("loads only the local runtime for local mode", async () => {
    let readyCalls = 0;
    const result = await loadRuntimeSnapshot("local", commands({
      readyState: async () => {
        readyCalls += 1;
        return readyState;
      },
    }));

    expect(result).toEqual({ snapshot: localSnapshot, readyState: null });
    expect(readyCalls).toBe(0);
  });

  test("preserves an absent remote runtime", async () => {
    const result = await loadRuntimeSnapshot("remote", commands({
      remoteState: async () => null,
    }));

    expect(result).toEqual({ snapshot: null, readyState: null });
  });

  test("loads the hosted readiness state and local runtime together", async () => {
    const calls: string[] = [];
    const result = await loadRuntimeSnapshot("zenith", commands({
      localState: async () => {
        calls.push("local");
        return localSnapshot;
      },
      readyState: async () => {
        calls.push("ready");
        return readyState;
      },
    }));

    expect(result).toEqual({ snapshot: localSnapshot, readyState });
    expect(calls.sort()).toEqual(["local", "ready"]);
  });
});
