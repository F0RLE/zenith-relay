import { relayCommands } from "../../api/commands";
import type { PoolRoutingEdit, PoolRoutingMember, RelayMode } from "../../api/types";
import { supportsRotationEditing } from "./rotationModes";

export type { PoolRoutingEdit } from "../../api/types";
export const routingMemberKey = (member: Pick<PoolRoutingMember, "kind" | "id">) => `${member.kind}:${member.id}`;

export const readRoutingRuntime = (mode: RelayMode) => mode === "local" ? relayCommands.localState() : Promise.resolve(null);

export async function persistPoolRoutingEdits(mode: RelayMode, edits: readonly PoolRoutingEdit[]) {
  for (let attempt = 0; ; attempt += 1) {
    const runtime = await readRoutingRuntime(mode);
    const currentPolicy = runtime?.gateway.poolRouting;
    if (!runtime || !supportsRotationEditing(currentPolicy, runtime.capabilities.features)) throw { code: "unsupported_schema", message: "pool rotation editing is unavailable in this runtime" };
    try {
      await relayCommands.editRouting(edits, currentPolicy);
    } catch (error) {
      const conflict = typeof error === "object" && error !== null && "code" in error
        && (error.code === "conflict" || error.code === "pool_routing_conflict");
      if (!conflict || attempt >= 2) throw error;
      continue;
    }
    const saved = await readRoutingRuntime(mode);
    if (!saved?.gateway.poolRouting || !saved.gateway.poolRotation) throw { code: "unsupported_schema", message: "pool rotation projection is unavailable" };
    return { policy: saved.gateway.poolRouting, rotation: saved.gateway.poolRotation };
  }
}
