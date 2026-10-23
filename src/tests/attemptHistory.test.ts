import { expect, test } from "bun:test";
import { loadRequestAttempts } from "../src/features/relay/pages/usage/attemptHistory";
import type { UsageRow } from "../src/features/relay/pages/usage/usageData";

const row = (id: number, attempt: number, requestId = "req"): UsageRow =>
  ({ id, attempt, requestId, time: "2026-10-10T10:00:00Z" } as UsageRow);

test("history retrieves all pages, excludes prefix collisions and keeps distinct records of an attempt", async () => {
  const result = await loadRequestAttempts("req", async (query) => {
    expect(query).toMatchObject({ requestId: "req", pageSize: 200, includeEvents: true, includeModels: false });
    expect(query.requestIdQuery).toBeUndefined();
    expect(query.success).toBeUndefined();
    expect(query.range).toBeUndefined();
    return { page: query.page!, totalPages: 2, rows: query.page === 1
      ? [row(3, 2), row(9, 1, "req-extra")]
      : [row(1, 1), row(2, 1), row(3, 2)] };
  });
  expect(result.complete).toBe(true);
  expect(result.rows.map((event) => event.id)).toEqual([1, 2, 3]);
});

test("a cancelled history discards late results and does not continue pagination", async () => {
  let active = true;
  let calls = 0;
  const result = await loadRequestAttempts("req", async () => {
    calls += 1;
    active = false;
    return { page: 1, totalPages: 2, rows: [row(1, 1)] };
  }, () => active);
  expect(calls).toBe(1);
  expect(result).toEqual({ rows: [], complete: false });
});

test("inconsistent and excessive pagination is explicitly incomplete", async () => {
  const inconsistent = await loadRequestAttempts("req", async () =>
    ({ page: 1, totalPages: 2, rows: [row(1, 1)] }));
  expect(inconsistent.complete).toBe(false);
  let pages = 0;
  const excessive = await loadRequestAttempts("req", async (query) => {
    pages += 1;
    return { page: query.page!, totalPages: 1000, rows: [] };
  });
  expect(pages).toBe(100);
  expect(excessive.complete).toBe(false);
});

test("failed history loads remain failures instead of becoming an empty success", async () => {
  await expect(loadRequestAttempts("req", async () => { throw new Error("offline"); })).rejects.toThrow("offline");
});
