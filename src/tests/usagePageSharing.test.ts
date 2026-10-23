import { describe, expect, test } from "bun:test";
import type { LocalUsagePage } from "../src/features/relay/api/types";
import { shareUsageFilterChoices } from "../src/features/relay/state/usagePageSharing";
import { emptyUsageTotals } from "../src/features/relay/usageTotals";

function report(fields: Partial<LocalUsagePage> = {}): LocalUsagePage {
  return {
    events: [], total: 0, page: 1, pageSize: 50, totalPages: 0,
    totals: emptyUsageTotals(), models: [], poolMembers: [], ...fields,
  };
}

describe("usage choice sharing", () => {
  test("repeated reports share every unchanged choice array, not request facts", () => {
    const original = report({ filterOptions: {
      models: Array.from({ length: 1_000 }, (_, index) => `model-${index}`),
      poolMembers: ["synthetic-member"], wireApis: ["responses"],
      transports: ["http"], errorCategories: ["upstream_failure"],
    } });
    const unsharedReports = [original];
    const sharedReports = [original];
    for (let page = 2; page <= 8; page += 1) {
      const next = structuredClone(original);
      next.page = page;
      next.totals.requests = page;
      const shared = shareUsageFilterChoices(next, original.filterOptions);
      unsharedReports.push(next);
      sharedReports.push(shared);
      expect(shared.filterOptions?.models).toBe(original.filterOptions?.models);
      expect(shared.filterOptions?.poolMembers).toBe(original.filterOptions?.poolMembers);
      expect(shared.filterOptions?.wireApis).toBe(original.filterOptions?.wireApis);
      expect(shared.filterOptions?.transports).toBe(original.filterOptions?.transports);
      expect(shared.filterOptions?.errorCategories).toBe(original.filterOptions?.errorCategories);
      expect(shared.events).toBe(next.events);
      expect(shared.totals).toBe(next.totals);
      expect(shared.page).toBe(page);
      expect(original.totals.requests).toBe(0);
    }
    expect(new Set(unsharedReports.map((page) => page.filterOptions?.models)).size).toBe(8);
    expect(new Set(sharedReports.map((page) => page.filterOptions?.models)).size).toBe(1);
  });

  test("new or reordered choices remain authoritative and do not mutate either report", () => {
    const original = report({ filterOptions: { models: ["a", "b"], poolMembers: ["old"] } });
    for (const models of [["a", "c"], ["b", "a"], []]) {
      const next = report({ filterOptions: { models, poolMembers: ["new"] } });
      const shared = shareUsageFilterChoices(next, original.filterOptions);
      expect(shared.filterOptions?.models).toBe(models);
      expect(shared.filterOptions?.poolMembers).toEqual(["new"]);
      expect(original.filterOptions).toEqual({ models: ["a", "b"], poolMembers: ["old"] });
    }
  });

  test("missing legacy fields remain missing, rather than borrowing old choices", () => {
    const older = report({ filterOptions: { models: ["a"], wireApis: ["responses"] } });
    const next = report({ filterOptions: { wireApis: ["responses"] } });
    const shared = shareUsageFilterChoices(next, older.filterOptions);
    expect(shared.filterOptions).not.toHaveProperty("models");
    expect(shared.filterOptions?.wireApis).toBe(older.filterOptions?.wireApis);
    const absent = report();
    expect(shareUsageFilterChoices(absent, older.filterOptions)).toBe(absent);
    expect(shareUsageFilterChoices(next, undefined)).toBe(next);
  });
});
