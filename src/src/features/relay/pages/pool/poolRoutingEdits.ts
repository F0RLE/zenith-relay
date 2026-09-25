import { relayCommands } from "../../api/commands";
import type { PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy, RelayMode } from "../../api/types";
import { persistRoutingPolicy } from "../../routingPolicy";

export type PoolRoutingEdit =
  | { type: "mode"; mode: PoolRoutingMode }
  | { type: "move"; member: string; target: string; placement: "before" | "after" }
  | { type: "member"; member: string; field: "weight" | "maxConcurrency"; value: number };

export const routingMemberKey = (member: PoolRoutingMember) => `${member.kind}:${member.id}`;

// Edits address existing identities, so refreshing cannot restore a removed member.
export function applyPoolRoutingEdits(policy: PoolRoutingPolicy, edits: readonly PoolRoutingEdit[]): PoolRoutingPolicy {
  return edits.reduce((current, edit) => {
    if (edit.type === "mode") return { ...current, mode: edit.mode };
    if (edit.type === "member") return {
      ...current,
      members: current.members.map((member) => routingMemberKey(member) === edit.member ? { ...member, [edit.field]: edit.value } : member),
    };
    if (current.mode !== "in_order" || edit.member === edit.target) return current;
    const member = current.members.find((entry) => routingMemberKey(entry) === edit.member);
    const members = current.members.filter((entry) => routingMemberKey(entry) !== edit.member);
    const target = members.findIndex((entry) => routingMemberKey(entry) === edit.target);
    if (!member || target < 0) return current;
    members.splice(target + (edit.placement === "after" ? 1 : 0), 0, member);
    return { ...current, members };
  }, policy);
}

export const readRoutingRuntime = (mode: RelayMode) => mode === "local" ? relayCommands.localState() : relayCommands.remoteState();

export async function persistPoolRoutingEdits(mode: RelayMode, edits: readonly PoolRoutingEdit[]) {
  for (let attempt = 0; ; attempt += 1) {
    const runtime = await readRoutingRuntime(mode);
    const current = runtime?.gateway.poolRouting;
    if (!runtime || !current) throw { code: "unsupported_schema", message: "pool routing is unavailable" };
    if (current.version !== 2 || !runtime.capabilities.features.includes("rotation_v2")) throw { code: "unsupported_schema", message: "pool rotation is unavailable on this server" };
    const next = applyPoolRoutingEdits(current, edits);
    if (JSON.stringify(next) === JSON.stringify(current)) return current;
    try {
      await persistRoutingPolicy(mode, {
        poolRouting: next,
        expectedPoolRouting: current,
        maxRetryCandidates: runtime.gateway.maxRetryCandidates,
        defaultServiceTier: runtime.gateway.defaultServiceTier,
      });
      return next;
    } catch (error) {
      const conflict = typeof error === "object" && error !== null && "code" in error
        && (error.code === "conflict" || error.code === "pool_routing_conflict");
      if (!conflict || attempt >= 2) throw error;
    }
  }
}
