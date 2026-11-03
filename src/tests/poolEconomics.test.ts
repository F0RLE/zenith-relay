import { describe, expect, test } from "bun:test";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { poolEconomics } from "../src/features/relay/pages/pool/poolEconomics";
import { calibratedWeek } from "./accountEconomics.fixture";

type PoolAccount = Extract<PoolMember, { kind: "account" }>;
type PoolSource = Extract<PoolMember, { kind: "source" }>;
const account = (overrides: Partial<PoolAccount> = {}): PoolAccount => ({
  kind: "account", id: "synthetic-account", label: "Account", identityHint: "Account",
  enabled: true, inPool: true, draining: false, operationalStatus: "rotation",
  authState: { state: "active" }, health: "healthy", models: [], allowedModels: [], excludedModels: [],
  priority: 1, weight: 1, secretAvailable: true, lastErrorCode: null, quotaRefreshStatus: "updated",
  subscription: { planType: null, activeUntilMs: null, status: "active", updatedAtMs: null },
  apiEquivalent: { microUsd: 20_000_000, pricedTokens: 100, unpricedTokens: 0 },
  purchaseCostMicroUsd: 10_000_000,
  quota: { primary: null, secondary: null, limitReached: false, updatedAtMs: null, error: null, directBalanceMicroUsd: 5_000_000 },
  ...overrides,
});
const source = (overrides: Partial<PoolSource> = {}): PoolSource => ({
  kind: "source", id: "synthetic-source", name: "Source", enabled: true, inPool: true,
  draining: false, operationalStatus: "rotation", baseUrl: "https://example.test/v1", wireApi: "responses",
  models: [], allowedModels: [], excludedModels: [], priority: 1, weight: 1,
  recoveryDelaySeconds: 0, secretAvailable: true, lastErrorCode: null,
  apiEquivalent: { microUsd: 80_000_000, pricedTokens: 400, unpricedTokens: 0 },
  ...overrides,
});

describe("pool economics", () => {
  test("sums member usage and weights payback by account purchase costs", () => {
    expect(poolEconomics([account(), account({
      id: "second-account",
      apiEquivalent: { microUsd: 30_000_000, pricedTokens: 150, unpricedTokens: 0 }, purchaseCostMicroUsd: 30_000_000,
    }), source()])).toMatchObject({
      used: { microUsd: 130_000_000, pricedTokens: 650, unpricedTokens: 0 },
      remainingMicroUsd: 10_000_000, remainingApproximate: false,
      purchaseMicroUsd: 40_000_000, recoveredMicroUsd: 50_000_000,
      payback: 1.25, paybackApproximate: false, accountCount: 2,
    });
  });

  test("excludes connections outside the pool and inactive routes, not paused pool members", () => {
    expect(poolEconomics([
      account({ enabled: false }), account({ inPool: false }), account({ routeInactive: true }), source({ inPool: false }),
    ])).toMatchObject({ used: { microUsd: 20_000_000 }, accountCount: 1, remainingMicroUsd: 5_000_000, payback: 2 });
  });

  test("marks partial coverage and excludes unknown-cost accounts from both sides of payback", () => {
    expect(poolEconomics([account(), account({ id: "unknown-account", purchaseCostMicroUsd: null, quota: { ...account().quota, directBalanceMicroUsd: null } })]))
      .toMatchObject({ used: { microUsd: 40_000_000 }, remainingMicroUsd: 5_000_000,
        remainingCount: 1, remainingApproximate: true, purchaseCount: 1, payback: 2, paybackApproximate: true });
  });

  test("keeps unknown or source-only remaining value and payback unavailable", () => {
    expect(poolEconomics([])).toMatchObject({ remainingMicroUsd: null, payback: null });
    expect(poolEconomics([source()])).toMatchObject({ remainingMicroUsd: null, payback: null });
    expect(poolEconomics([account({ purchaseCostMicroUsd: null, quota: { ...account().quota, directBalanceMicroUsd: null } })]))
      .toMatchObject({ remainingMicroUsd: null, payback: null });
    expect(poolEconomics([account({ purchaseCostMicroUsd: 0 })])).toMatchObject({ purchaseCount: 1, payback: null });
  });

  test("preserves measured zero and incomplete pricing without inventing zero payback", () => {
    expect(poolEconomics([account({ apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 }, quota: { ...account().quota, directBalanceMicroUsd: 0 } })]))
      .toMatchObject({ remainingMicroUsd: 0, payback: 0, paybackApproximate: false });
    expect(poolEconomics([account({ apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 10 } })]))
      .toMatchObject({ payback: null, paybackApproximate: true });
    expect(poolEconomics([account({ apiEquivalent: { microUsd: 20_000_000, pricedTokens: 100, unpricedTokens: 10 } })]))
      .toMatchObject({ payback: 2, paybackApproximate: true });
  });

  test("retains the existing quota observation evidence gates", () => {
    const now = Date.now();
    const secondary = { kind: "secondary" as const, availableBasisPoints: 5_000, explicitlyFull: false, resetAtMs: now + 6 * 86_400_000, windowStartMs: now - 86_400_000, windowMinutes: 10_080, observedAtMs: now };
    const quota = { ...account().quota, directBalanceMicroUsd: null, secondary };
    const quotaWindowUsage = calibratedWeek(now, { availableBasisPoints: 5_000 });
    expect(poolEconomics([account({ quota, quotaWindowUsage })])).toMatchObject({ remainingMicroUsd: 5_000_000, remainingApproximate: true });
    expect(poolEconomics([account({ quota, quotaWindowUsage: { ...quotaWindowUsage, observedAtMs: 9_999 } })]))
      .toMatchObject({ remainingMicroUsd: null });
  });

  test("linked connections contribute real usage but only one remaining subscription", () => {
    const quotaWindowUsage = calibratedWeek(Date.now(), { availableBasisPoints: 8_400 });
    const quota = { ...account().quota, directBalanceMicroUsd: null };
    const members = ["codex", "bps", "siwc"].map((id) => account({ id, quota, quotaWindowUsage, creditBalanceKey: "synthetic-ledger" }));
    expect(poolEconomics(members)).toMatchObject({ used: { microUsd: 60_000_000 }, remainingMicroUsd: 8_400_000,
      remainingCount: 1, remainingTotalCount: 1, accountCount: 3 });
  });

  test("pool sums shared median estimates once per proven ledger", () => {
    const quotaWindowUsage = calibratedWeek(Date.now());
    quotaWindowUsage.calibration = { ...quotaWindowUsage.calibration,
      profileCount: 2, workload: "", medianMicroUsd: 8_500_000, cacheBand: null, outputBand: null };
    const quota = { ...account().quota, directBalanceMicroUsd: null };
    const linked = ["codex", "bps"].map((id) => account({ id, quota, quotaWindowUsage }));
    const total = poolEconomics([...linked, account({ id: "cash" })]);
    expect(total.remainingCount).toBe(2);
    expect(total.remainingMicroUsd).toBe(9_930_000);
    expect(total.remainingEstimate).toEqual({ microUsd: 9_930_000, approximate: true });
  });

  test("direct balances deduplicate only proven identities and use the freshest observation", () => {
    const first = account({ id: "first", creditBalanceKey: "shared", quota: { ...account().quota, updatedAtMs: 1 } });
    const next = account({ id: "next", creditBalanceKey: "shared", quota: { ...account().quota, updatedAtMs: 2, directBalanceMicroUsd: 3_000_000 } });
    expect(poolEconomics([first, next])).toMatchObject({ remainingMicroUsd: 3_000_000, remainingTotalCount: 1 });
    expect(poolEconomics([next, first])).toMatchObject({ remainingMicroUsd: 3_000_000 });
    expect(poolEconomics([account({ id: "a" }), account({ id: "b" })])).toMatchObject({ remainingMicroUsd: 10_000_000, remainingTotalCount: 2 });
  });

  test("expires pool forecasts even if the member array has not changed", () => {
    const now = Date.now();
    const quotaWindowUsage = calibratedWeek(now);
    const members = [account({ quota: { ...account().quota, directBalanceMicroUsd: null }, quotaWindowUsage })];
    expect(poolEconomics(members, now).remainingMicroUsd).toBe(5_800_000);
    expect(poolEconomics(members, quotaWindowUsage.calibration.validUntilMs).remainingMicroUsd).toBeNull();
  });
});
