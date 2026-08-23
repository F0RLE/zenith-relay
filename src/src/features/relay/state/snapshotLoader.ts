import type { UiState } from "../api/commands";
import type { RelayMode, RuntimeSnapshot } from "../api/types";

export type RuntimeSnapshotCommands = {
  localState: () => Promise<RuntimeSnapshot>;
  remoteState: () => Promise<RuntimeSnapshot | null>;
  readyState: () => Promise<UiState>;
};

export type LoadedRuntimeSnapshot = {
  snapshot: RuntimeSnapshot | null;
  readyState: UiState | null;
};

/** Keep mode-specific IPC branching outside the provider's state orchestration. */
export async function loadRuntimeSnapshot(
  mode: RelayMode,
  commands: RuntimeSnapshotCommands,
): Promise<LoadedRuntimeSnapshot> {
  if (mode === "local") {
    return { snapshot: await commands.localState(), readyState: null };
  }
  if (mode === "remote") {
    return { snapshot: await commands.remoteState(), readyState: null };
  }
  const [readyState, snapshot] = await Promise.all([
    commands.readyState(),
    commands.localState(),
  ]);
  return { snapshot, readyState };
}
