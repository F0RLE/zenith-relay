import { describe, expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, RuntimeSnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import {
  orderedPoolMembers,
  poolActivityState,
  poolMemberRuntimeStates,
  poolMembersFromRuntime,
  poolMemberSourceIds,
  poolMemberStatusCounts,
} from "../src/features/relay/pages/pool/poolMembersModel";

const member = (kind: "account" | "source", id: string, overrides: Record<string, unknown> = {}) => ({
  kind,
  id,
  inPool: true,
  enabled: true,
  operationalStatus: "rotation",
  priority: kind === "source" ? 1 : undefined,
  lastErrorCode: null,
  ...(kind === "account" ? { authState: { state: "ready" }, quota: {}, quotaRefreshStatus: "updated" } : {}),
  ...overrides,
} as unknown as PoolMember);

const candidate = (candidateId: string, overrides: Record<string, unknown> = {}) => ({
  candidateId,
  kind: candidateId.startsWith("source") ? "api_source" : "oauth_account",
  available: true,
  inFlight: 0,
  activeRequestCount: 0,
  dispatches: 0,
  ...overrides,
} as CandidateRuntimeSnapshot);

describe("pool members model", () => {
  test("projects only pooled accounts and sources and keeps source ids stable", () => {
    const runtime = {
      accounts: [{ id: "account-1", inPool: true }, { id: "account-2", inPool: false }],
      sources: [{ id: "source-2", inPool: true }, { id: "source-1", inPool: true }],
    } as unknown as RuntimeSnapshot;
    const members = poolMembersFromRuntime(runtime);
    expect(members.map((item) => `${item.kind}:${item.id}`)).toEqual([
      "account:account-1",
      "source:source-2",
      "source:source-1",
    ]);
    expect(poolMemberSourceIds(members)).toBe("source-1\nsource-2");
  });

  test("maps protocol candidates back to source cards and preserves backend order", () => {
    const members = [member("source", "source-1"), member("account", "account-1")];
    const order = [candidate("account-1"), candidate("source-1::responses")];
    const runtimeByMember = poolMemberRuntimeStates(members, order);
    expect(runtimeByMember.get("source-1")?.candidateId).toBe("source-1");
    expect(orderedPoolMembers(members, order).map((item) => item.id)).toEqual(["account-1", "source-1"]);
  });

  test("derives active, last-used, and next route state from runtime snapshots", () => {
    const members = [member("account", "active"), member("source", "source-last"), member("source", "source-next")];
    const order = [
      candidate("active", { available: false, activeRequestCount: 2, inFlight: 2, activeModels: [{ model: "gpt-5.4", requestCount: 2 }] }),
      candidate("source-last::responses", { available: false, lastUsedAtMs: 20 }),
      candidate("source-next::responses", { available: true }),
    ];
    const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order);
    expect(state.activeMembers.map((item) => item.id)).toEqual(["active"]);
    expect(state.activeRequestTotal).toBe(2);
    expect(state.activeModels).toEqual([{ model: "gpt-5.4", requestCount: 2 }]);
    expect(state.lastUsedMember?.id).toBe("source-last");
    expect(state.nextMember?.id).toBe("source-next");
  });

  test("counts account and source errors without changing status semantics", () => {
    const members = [
      member("account", "ready"),
      member("account", "error", { quotaRefreshStatus: "failed", quota: { error: { code: "quota_transport" } } }),
      member("source", "source-error", { operationalStatus: "unavailable", lastErrorCode: "upstream_404" }),
      member("source", "disabled", { operationalStatus: "disabled" }),
      member("account", "quota", { operationalStatus: "quotaWait" }),
    ];
    expect(poolMemberStatusCounts(members)).toEqual({ rotation: 2, quotaWait: 1, errors: 2, disabled: 1 });
  });
});
