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
  return edits.reduce((workingPolicy, edit) => {
    if (edit.type === "mode") return { ...workingPolicy, mode: edit.mode };
    if (edit.type === "member") return {
      ...workingPolicy,
      members: workingPolicy.members.map((member) => routingMemberKey(member) === edit.member ? { ...member, [edit.field]: edit.value } : member),
    };
    if (workingPolicy.mode === "automatic" || edit.member === edit.target) return workingPolicy;
    const member = workingPolicy.members.find((candidateMember) => routingMemberKey(candidateMember) === edit.member);
    const members = workingPolicy.members.filter((candidateMember) => routingMemberKey(candidateMember) !== edit.member);
    const targetIndex = members.findIndex((candidateMember) => routingMemberKey(candidateMember) === edit.target);
    if (!member || targetIndex < 0) return workingPolicy;
    members.splice(targetIndex + (edit.placement === "after" ? 1 : 0), 0, member);
    return { ...workingPolicy, members };
  }, policy);
}

export const readRoutingRuntime = (mode: RelayMode) => mode === "local" ? relayCommands.localState() : relayCommands.remoteState();

export async function persistPoolRoutingEdits(mode: RelayMode, edits: readonly PoolRoutingEdit[]) {
  for (let attempt = 0; ; attempt += 1) {
    const runtime = await readRoutingRuntime(mode);
    const currentPolicy = runtime?.gateway.poolRouting;
    if (!runtime || !currentPolicy) throw { code: "unsupported_schema", message: "pool routing is unavailable" };
    if (currentPolicy.version !== 2 || !runtime.capabilities.features.includes("rotation_v2")) throw { code: "unsupported_schema", message: "pool rotation is unavailable on this server" };
    const updatedPolicy = applyPoolRoutingEdits(currentPolicy, edits);
    if (JSON.stringify(updatedPolicy) === JSON.stringify(currentPolicy)) return currentPolicy;
    try {
      await persistRoutingPolicy(mode, {
        poolRouting: updatedPolicy,
        expectedPoolRouting: currentPolicy,
        maxRetryCandidates: runtime.gateway.maxRetryCandidates,
        defaultServiceTier: runtime.gateway.defaultServiceTier,
      });
      return updatedPolicy;
    } catch (error) {
      const conflict = typeof error === "object" && error !== null && "code" in error
        && (error.code === "conflict" || error.code === "pool_routing_conflict");
      if (!conflict || attempt >= 2) throw error;
    }
  }
}
