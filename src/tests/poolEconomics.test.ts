import { describe, expect, test } from "bun:test";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { poolEconomics } from "../src/features/relay/pages/pool/poolEconomics";

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
    expect(poolEconomics([account(), account({ purchaseCostMicroUsd: null, quota: { ...account().quota, directBalanceMicroUsd: null } })]))
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
    const quotaWindowUsage = { kind: "secondary" as const, windowStartMs: secondary.windowStartMs, observedAtMs: now, windowMinutes: 10_080, apiEquivalent: { microUsd: 5_000_000, pricedTokens: 100, unpricedTokens: 0 } };
    expect(poolEconomics([account({ quota, quotaWindowUsage })])).toMatchObject({ remainingMicroUsd: 5_000_000, remainingApproximate: true });
    expect(poolEconomics([account({ quota, quotaWindowUsage: { ...quotaWindowUsage, observedAtMs: 9_999 } })]))
      .toMatchObject({ remainingMicroUsd: null });
  });
});
