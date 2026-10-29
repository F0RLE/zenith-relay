import { describe, expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, RuntimeActivitySnapshot } from "../src/features/relay/api/types";
import { createRuntimeCandidateLookup, indexCandidatesByMember } from "../src/features/relay/runtimeCandidateIndex";
import { runtimeCandidateForMember } from "../src/features/relay/routingOrder";

function candidate(candidateId: string, kind: CandidateRuntimeSnapshot["kind"], active = false): CandidateRuntimeSnapshot {
  return {
    candidateId, kind, available: true, inFlight: Number(active), activeRequestCount: Number(active),
    activeModels: active ? [{ model: "synthetic-model", requestCount: 1 }] : [],
    dispatches: 1, lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false,
  };
}

describe("runtime candidate index", () => {
  test("preserves aggregation, protocol filters and exact kind across overlapping IDs", () => {
    const order = [
      candidate("shared", "oauth_account"), candidate("shared::messages", "api_source", true),
      candidate("shared::responses", "api_source"), candidate("shared", "api_source"),
      candidate("shared-extra::responses", "api_source", true),
      candidate("shared::nested::responses", "api_source", true),
      candidate("shared::messages", "oauth_account"),
    ];
    const original = structuredClone(order);
    const lookup = createRuntimeCandidateLookup(order);
    for (const id of ["shared", "shared-extra", "shared::nested", "shared::messages", "missing"]) {
      for (const kind of ["oauth_account", "api_source"] as const) {
        for (const protocol of ["all", "responses"] as const) {
          for (const wire of ["messages", "responses", undefined] as const) {
            expect(lookup(id, kind, protocol, wire)).toEqual(runtimeCandidateForMember(id, kind, order, protocol, wire));
          }
        }
      }
    }
    expect(lookup("shared", "oauth_account")).toBe(order[0]);
    expect(order).toEqual(original);
  });

  test("indexes kindless activity by exact account ID and all source prefixes", () => {
    const activity = (candidateId: string): RuntimeActivitySnapshot => ({
      candidateId, runtimeId: 1, revision: 1, inFlight: 1, activeRequestCount: 1, activeModels: [],
    });
    const activities = [activity("one::responses"), activity("one::nested::messages"), activity("one"), activity("one-more")];
    const lookup = indexCandidatesByMember(activities);
    expect(lookup("one", "oauth_account")).toEqual([activities[2]]);
    expect(lookup("one", "api_source")).toEqual(activities.slice(0, 3));
    expect(lookup("one::nested", "api_source")).toEqual([activities[1]]);
    expect(lookup("missing", "api_source")).toEqual([]);
  });

  test("indexes are scoped to the supplied snapshot, not a process-global cache", () => {
    const before = candidate("one", "oauth_account");
    const after = { ...before, available: false };
    expect(createRuntimeCandidateLookup([before])("one", "oauth_account")).toBe(before);
    expect(createRuntimeCandidateLookup([after])("one", "oauth_account")).toBe(after);
    expect(createRuntimeCandidateLookup([])("one", "oauth_account")).toBeUndefined();
  });
});
