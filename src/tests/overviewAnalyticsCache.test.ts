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

  test("a backwards clock adjustment does not make an old report fresh", () => {
    const scope = `cache-clock-test:${Date.now()}`;
    rememberOverviewAnalytics(scope, { totals: emptyUsageTotals(), buckets: [] });
    expect(isOverviewAnalyticsFresh(scope, Date.now() - 60_000)).toBe(false);
  });

  test("usage arriving during a read schedules one fresh read after it", async () => {
    const scope = `cache-trailing-test:${Date.now()}`;
    const older = { totals: emptyUsageTotals(), buckets: [] };
    const newer = { totals: { ...emptyUsageTotals(), requests: 2 }, buckets: [] };
    let finish!: (value: typeof older) => void;
    const first = loadOverviewAnalytics(scope, () => new Promise<typeof older>((resolve) => { finish = resolve; }));
    let refreshes = 0;
    const refresh = () => { refreshes += 1; return Promise.resolve(newer); };
    const next = loadOverviewAnalytics(scope, refresh, true);
    const deduplicated = loadOverviewAnalytics(scope, refresh, true);
    finish(older);
    await expect(first).resolves.toBe(older);
    await expect(next).resolves.toBe(newer);
    await expect(deduplicated).resolves.toBe(newer);
    expect(refreshes).toBe(1);
    expect(getCachedOverviewAnalytics(scope)).toBe(newer);
  });

  test("a trailing refresh still runs when the superseded read fails", async () => {
    const scope = `cache-trailing-error-test:${Date.now()}`;
    const analytics = { totals: emptyUsageTotals(), buckets: [] };
    let fail!: (reason: Error) => void;
    const first = loadOverviewAnalytics(scope, () => new Promise<never>((_, reject) => { fail = reject; }));
    const caught = first.catch(() => null);
    const next = loadOverviewAnalytics(scope, () => Promise.resolve(analytics), true);
    fail(new Error("Synthetic superseded read failure"));
    await caught;
    await expect(next).resolves.toBe(analytics);
    expect(getCachedOverviewAnalytics(scope)).toBe(analytics);
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

  for (const fail of [false, true]) {
    test(`leaving Overview cancels queued work after a pending read ${fail ? "fails" : "succeeds"}`, async () => {
      const scope = `cache-navigation-test:${fail}:${Date.now()}`;
      const analytics = { totals: emptyUsageTotals(), buckets: [] };
      let finish!: () => void;
      const first = loadOverviewAnalytics(scope, () => new Promise<typeof analytics>((resolve, reject) => {
        finish = () => fail ? reject(new Error("Synthetic read failure")) : resolve(analytics);
      })).catch(() => null);
      const controller = new AbortController();
      let refreshes = 0;
      const next = loadOverviewAnalytics(scope, () => { refreshes += 1; return Promise.resolve(analytics); }, true, controller.signal)
        .catch((error: unknown) => error);
      controller.abort();
      finish();
      await first;
      expect(await next).toMatchObject({ name: "AbortError" });
      expect(refreshes).toBe(0);
    });
  }
});
