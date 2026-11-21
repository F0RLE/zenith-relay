import { expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, RuntimeActivitySnapshot } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { poolActivityState, poolMemberRuntimeStates, poolRoutingAvailability } from "../src/features/relay/pages/pool/poolMembersModel";

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

test("availability and next-route checks normalize the catalog once per projection", () => {
  const { members, order } = poolFixture(500);
  for (const member of members) Object.assign(member, {
    inPool: true, enabled: true, operationalStatus: "rotation",
    secretAvailable: true, proxyAvailable: true, models: ["not-in-catalog"],
  });
  const catalog = Array.from({ length: 512 }, (_, index) => `model-${index}`);
  let reads = 0;
  for (let index = 0; index < catalog.length; index += 1) {
    const model = catalog[index];
    Object.defineProperty(catalog, index, { get() { reads += 1; return model; } });
  }
  expect(poolRoutingAvailability(members, catalog, 0)).toBe("noModels");
  expect(reads).toBe(catalog.length);

  members[499]!.models = ["MODEL-511"];
  for (const candidate of order) candidate.nextForNewRequest = true;
  const state = poolActivityState(members, poolMemberRuntimeStates(members, order), order, undefined, catalog);
  expect(state.nextMember).toBe(members[499]);
  expect(reads).toBe(catalog.length * 2);

  // A new projection uses the current inventory, not a cross-snapshot cache.
  expect(poolRoutingAvailability(members, ["other-model"], 0)).toBe("noModels");
  expect(poolRoutingAvailability(members, [], 0)).toBe("noModels");
  expect(poolRoutingAvailability(members, catalog, 0)).toBe("ready");
  expect(reads).toBe(catalog.length * 3);
});

test("active or unavailable pools and absent previews never read the visible catalog", () => {
  const { members, order } = poolFixture(3);
  const catalog = ["synthetic"];
  Object.defineProperty(catalog, 0, { get() { throw new Error("catalog read without an eligible candidate"); } });
  expect(poolRoutingAvailability(members, catalog, 1)).toBe("active");
  expect(poolRoutingAvailability(members, catalog, 0)).toBe("unavailable");
  for (const member of members) Object.assign(member, {
    inPool: true, enabled: true, operationalStatus: "rotation",
    secretAvailable: true, proxyAvailable: true, models: ["synthetic"],
  });
  expect(poolActivityState(members, poolMemberRuntimeStates(members, order), order, undefined, catalog).nextMember).toBeNull();
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
