import type { RelayMode, RuntimeSnapshot } from "../api/types";

export type RuntimeSnapshotCommands = {
  localState: () => Promise<RuntimeSnapshot>;
};

/** Hosted never opens a local runtime or a retired server connection. */
export async function loadRuntimeSnapshot(
  mode: RelayMode,
  commands: RuntimeSnapshotCommands,
): Promise<RuntimeSnapshot | null> {
  if (mode === "hosted") return null;
  return commands.localState();
}
