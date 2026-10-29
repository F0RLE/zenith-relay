import { describe, expect, test } from "bun:test";
import type { LocalUsagePage, UsageQuery } from "../src/features/relay/api/types";
import { createUsagePageReader } from "../src/features/relay/api/usagePage";
import { emptyUsageTotals } from "../src/features/relay/usageTotals";

function page(fields: Partial<LocalUsagePage> = {}): LocalUsagePage {
  return {
    events: [], total: 0, page: 1, pageSize: 50, totalPages: 0,
    totals: emptyUsageTotals(), models: [], poolMembers: [], ...fields,
  };
}

const history = () => page({
  models: ["historical-model", "", "historical-model"].map((key) => ({ key, totals: emptyUsageTotals() })),
  poolMembers: ["historical-member"].map((key) => ({ key, totals: emptyUsageTotals() })),
});

describe("usage filter compatibility", () => {
  test("does not mislabel an old host's unfiltered totals as memory usage", async () => {
    const load = createUsagePageReader(async () => page());
    await expect(load({ workload: "memory" })).rejects.toThrow("workload-scoped");
    await expect(load({})).resolves.toEqual(page());
  });

  test("accepts a workload only when the host confirms the applied scope", async () => {
    const load = createUsagePageReader(async (query) => page({ workload: query.workload }));
    const result = await load({ workload: "conversation", includeEvents: false });
    expect(result.workload).toBe("conversation");
  });

  test("does not confuse a complete empty projection with an unsupported one", async () => {
    const current = page({ filterOptions: { models: [], poolMembers: [], wireApis: [], transports: [], errorCategories: [] } });
    let reads = 0;
    const load = createUsagePageReader(async () => { reads += 1; return current; });
    expect(await load({ includeFilterOptions: true })).toBe(current);
    expect(reads).toBe(1);
  });

  for (const advanced of [false, true]) {
    test(`loads whole-period aggregates when the host omits model/member fields: advanced=${advanced}`, async () => {
      const original = page({
        page: 4, total: 1_000, totalPages: 20,
        ...(advanced ? { filterOptions: { wireApis: ["messages"], transports: ["http"], errorCategories: ["upstream_failure"] } } : {}),
      });
      const reads: UsageQuery[] = [];
      const load = createUsagePageReader(async (query) => {
        reads.push(query);
        return query.includeFilterOptions ? original : history();
      });
      const result = await load({
        page: 4, pageSize: 50, range: "custom", fromMs: 10, toMs: 20, bucketMs: 60_000,
        modelQuery: "unmatched", sourceOrAccountQuery: "unmatched", success: false,
        wireApi: "responses", transport: "websocket", errorCategory: "unmatched", requestIdQuery: "unmatched",
        includeEvents: true, includeModels: false, includePoolMembers: false, includeFilterOptions: true,
      });
      expect(reads).toHaveLength(2);
      expect(reads[1]).toEqual({
        range: "custom", fromMs: 10, toMs: 20,
        includeEvents: false, includeModels: true, includePoolMembers: true,
      });
      expect(result.events).toBe(original.events);
      expect(result.totals).toBe(original.totals);
      expect([result.page, result.total, result.totalPages]).toEqual([4, 1_000, 20]);
      expect(result.filterOptions?.models).toEqual(["historical-model"]);
      expect(result.filterOptions?.poolMembers).toEqual(["historical-member"]);
      expect(result.filterOptions?.wireApis).toEqual(advanced ? ["messages"] : undefined);
    });
  }

  test("preserves a supported model projection when only member choices are missing", async () => {
    const original = page({ filterOptions: { models: [] } });
    const load = createUsagePageReader(async (query) => query.includeFilterOptions ? original : history());
    const result = await load({ includeFilterOptions: true });
    expect(result.filterOptions?.models).toEqual([]);
    expect(result.filterOptions?.poolMembers).toEqual(["historical-member"]);
  });

  test("does not load aggregates for ordinary reports or attempt history", async () => {
    let reads = 0;
    const load = createUsagePageReader(async () => { reads += 1; return page(); });
    await load();
    await load({ requestId: "synthetic-request", includeEvents: true });
    expect(reads).toBe(2);
  });

  test("joins simultaneous compatibility reads for the same period", async () => {
    let historyReads = 0;
    const load = createUsagePageReader(async (query) => {
      if (query.includeFilterOptions) return page();
      historyReads += 1;
      await new Promise((resolve) => setTimeout(resolve, 0));
      return history();
    });
    const results = await Promise.all([
      load({ range: "weekly", modelQuery: "first", includeFilterOptions: true }),
      load({ range: "weekly", modelQuery: "second", includeFilterOptions: true }),
    ]);
    expect(historyReads).toBe(1);
    expect(results.map((result) => result.filterOptions?.models)).toEqual([["historical-model"], ["historical-model"]]);
  });

  test("reports a failed aggregate read instead of presenting a false empty list", async () => {
    const load = createUsagePageReader(async (query) => {
      if (query.includeFilterOptions) return page();
      throw new Error("synthetic aggregate failure");
    });
    await expect(load({ includeFilterOptions: true })).rejects.toThrow("synthetic aggregate failure");
  });
});
