import { describe, expect, test } from "bun:test";
import { emptyUsageTotals } from "../src/features/relay/usageTotals";
import { getCachedOverviewAnalytics, isOverviewAnalyticsFresh, loadOverviewAnalytics, rememberOverviewAnalytics } from "../src/features/relay/pages/overview/overviewAnalyticsCache";

describe("overview analytics cache", () => {
  test("returns the last complete snapshot for a scope", () => {
    const scope = `cache-test:${Date.now()}`;
    const analytics = { totals: emptyUsageTotals(), buckets: [] };

    expect(getCachedOverviewAnalytics(scope)).toBeNull();
    rememberOverviewAnalytics(scope, analytics);
    expect(getCachedOverviewAnalytics(scope)).toBe(analytics);
  });

  test("keeps only a small recent set of scopes", () => {
    const prefix = `cache-limit-test:${Date.now()}`;
    const analytics = { totals: emptyUsageTotals(), buckets: [] };
    const scopes = Array.from({ length: 9 }, (_, index) => `${prefix}:${index}`);

    scopes.forEach((scope) => rememberOverviewAnalytics(scope, analytics));

    expect(getCachedOverviewAnalytics(scopes[0])).toBeNull();
    expect(getCachedOverviewAnalytics(scopes.at(-1)!)).toBe(analytics);
  });

  test("deduplicates concurrent refreshes for one scope", async () => {
    const scope = `cache-inflight-test:${Date.now()}`;
    const analytics = { totals: emptyUsageTotals(), buckets: [] };
    let loads = 0;
    let resolve: ((value: typeof analytics) => void) | undefined;
    const loader = () => {
      loads += 1;
      return new Promise<typeof analytics>((done) => { resolve = done; });
    };

    const first = loadOverviewAnalytics(scope, loader);
    const second = loadOverviewAnalytics(scope, loader);
    expect(first).toBe(second);
    expect(loads).toBe(1);
    resolve?.(analytics);
    await expect(first).resolves.toBe(analytics);
    expect(getCachedOverviewAnalytics(scope)).toBe(analytics);
    expect(isOverviewAnalyticsFresh(scope)).toBe(true);
  });
});
