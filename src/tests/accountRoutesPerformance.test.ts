import { expect, test } from "bun:test";
import type { AccountSummary } from "../src/features/relay/api/types";
import { accountRouteConnections, expandAccountRouteIds } from "../src/features/relay/accountRoutes";

function inventory(count: number) {
  return Array.from({ length: count }, (_, index) => ({
    id: `account-${index}`, routeGroupId: `group-${Math.floor(index / 3)}`,
  } as AccountSummary));
}

test("linked connection lookups read one inventory per immutable snapshot", () => {
  const accounts = inventory(600);
  let visits = 0;
  const snapshot = new Proxy(accounts, {
    get(target, key, receiver) {
      if (typeof key === "string" && /^\d+$/.test(key)) visits += 1;
      return Reflect.get(target, key, receiver);
    },
  });
  for (let tick = 0; tick < 2; tick += 1) {
    for (const account of accounts) {
      const connections = accountRouteConnections(account, snapshot);
      expect(connections).toHaveLength(3);
      expect(connections).toContain(account);
    }
  }
  expect(expandAccountRouteIds(accounts.map(({ id }) => id), snapshot)).toEqual(accounts.map(({ id }) => id));
  expect(visits).toBeLessThanOrEqual(accounts.length);
});

test("a new route snapshot replaces the index and caller edits cannot corrupt its groups", () => {
  const accounts = inventory(6);
  const selected = accounts[0]!;
  accountRouteConnections(selected, accounts).pop();
  expect(accountRouteConnections(selected, accounts).map(({ id }) => id)).toEqual(["account-0", "account-1", "account-2"]);
  const moved = accounts.map((account) => account.id === "account-2" ? { ...account, routeGroupId: "other" } : account);
  expect(accountRouteConnections(selected, moved).map(({ id }) => id)).toEqual(["account-0", "account-1"]);
  expect(accountRouteConnections(selected, accounts).map(({ id }) => id)).toEqual(["account-0", "account-1", "account-2"]);
  expect(expandAccountRouteIds(["account-0", "missing", "account-1"], moved)).toEqual(["account-0", "account-1", "missing"]);
});

const measure = process.env.RELAY_MEASURE_ACCOUNT_ROUTES === "1" ? test : test.skip;
measure("measure linked connection lookup with synthetic inventories", () => {
  for (const count of [600, 1200]) {
    const accounts = inventory(count);
    for (let warmup = 0; warmup < 5; warmup += 1) {
      for (const account of accounts) accountRouteConnections(account, accounts);
    }
    const samples: number[] = [];
    for (let repeat = 0; repeat < 30; repeat += 1) {
      const start = performance.now();
      let connections = 0;
      for (const account of accounts) connections += accountRouteConnections(account, accounts).length;
      samples.push(performance.now() - start);
      expect(connections).toBe(count * 3);
    }
    samples.sort((a, b) => a - b);
    console.info(JSON.stringify({ accounts: count, samples: samples.length, medianMs: samples[15], p95Ms: samples[28] }));
  }
});
