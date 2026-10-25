import { expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, RuntimeActivitySnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { poolMemberRuntimeStates } from "../src/features/relay/pages/pool/poolMembersModel";

function poolFixture(count: number) {
  const members = Array.from({ length: count }, (_, index) => ({
    id: `account-${index}`, kind: "account",
  } as PoolMember));
  const order: CandidateRuntimeSnapshot[] = members.map(({ id }) => ({
    candidateId: id, kind: "oauth_account", available: true,
    inFlight: 0, activeRequestCount: 0, dispatches: 0,
    lastUsedAtMs: null, nextRetryAtMs: null, halfOpen: false,
  }));
  return { members, order };
}

test("pool projection visits candidates linearly instead of scanning the pool per member", () => {
  const { members, order } = poolFixture(500);
  let candidateVisits = 0;
  for (const candidate of order) Object.defineProperty(candidate, "kind", {
    get() { candidateVisits += 1; return "oauth_account"; },
  });
  const projected = poolMemberRuntimeStates(members, order);
  expect(projected.size).toBe(members.length);
  for (const candidate of order) expect(projected.get(candidate.candidateId)).toBe(candidate);
  console.info(`Pool projection: ${members.length} members, ${candidateVisits} candidate-kind reads`);
  expect(candidateVisits).toBeLessThanOrEqual(members.length * 4);
});

test("pool projection reads each activity record once for overlay and member lookup", () => {
  const { members, order } = poolFixture(4);
  const event: RuntimeActivitySnapshot = {
    candidateId: "account-0", revision: 1, inFlight: 1,
    activeRequestCount: 1, activeModels: [{ model: "gpt", requestCount: 1 }],
  };
  const candidates: Record<string, RuntimeActivitySnapshot> = {};
  let reads = 0;
  Object.defineProperty(candidates, event.candidateId, {
    enumerable: true,
    get() { reads += 1; return event; },
  });
  const projected = poolMemberRuntimeStates(members, order, {
    revision: 1, lastCandidateId: event.candidateId, candidates,
  });
  expect(projected.get(event.candidateId)?.activeRequestCount).toBe(1);
  expect(reads).toBe(1);
});

const measure = process.env.RELAY_MEASURE_POOL_PROJECTION === "1" ? test : test.skip;
measure("measure pool projection with synthetic inventories", () => {
  for (const size of [500, 1000]) {
    const { members, order } = poolFixture(size);
    for (let warmup = 0; warmup < 20; warmup += 1) poolMemberRuntimeStates(members, order);
    const samples: number[] = [];
    for (let index = 0; index < 100; index += 1) {
      const start = performance.now();
      const projected = poolMemberRuntimeStates(members, order);
      samples.push(performance.now() - start);
      expect(projected.size).toBe(size);
    }
    samples.sort((left, right) => left - right);
    console.info(JSON.stringify({ members: size, samples: samples.length,
      medianMs: samples[50], p95Ms: samples[95] }));
  }
});
