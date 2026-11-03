import type { OwnerAccountPresentation } from "../features/owner/accountPresentation";

// Synthetic presentation fixtures. Values reuse Desktop accountEconomics.fixture.ts,
// accountEconomics.test.ts and accountStatus.test.ts cases, anchored to preview.asOf.
// The 37-day subscription is the Desktop e2e/tauri-mock/runtime.ts default fixture.
// Never populated from local accounts, credentials or the user's screenshots.
export function previewOwnerAccount(nowMs: number, variant: "normal" | "low"): OwnerAccountPresentation {
  const weekly = {
    kind: "secondary" as const, availableBasisPoints: 5800, explicitlyFull: false,
    resetAtMs: nowMs + 6 * 86400000, windowMinutes: 10080, observedAtMs: nowMs,
  };
  return {
    plan: "Plus", route: variant === "normal" ? "Excel / BPS" : "SIWC",
    quota: { primary: { ...weekly, kind: "primary", availableBasisPoints: variant === "normal" ? 7500 : 2000, resetAtMs: nowMs + 3600000, windowMinutes: 300 }, secondary: weekly },
    quotaStatus: "updated", subscriptionUntilMs: nowMs + 37 * 86400000,
    apiEquivalent: { microUsd: variant === "normal" ? 10000000 : 90000000, unpricedTokens: 0 },
    purchaseCostMicroUsd: variant === "normal" ? 5000000 : null,
    quotaWindowUsage: {
      ledgerKey: `synthetic-hosted-${variant}`, kind: "secondary", windowStartMs: nowMs - 86400000,
      resetAtMs: weekly.resetAtMs, observedAtMs: nowMs, windowMinutes: 10080, availableBasisPoints: 5800,
      calibration: {
        profileId: "synthetic-shared-profile", workload: "synthetic-model · default", cacheBand: 75, outputBand: 10,
        medianMicroUsd: 10000000, lowMicroUsd: 8000000, highMicroUsd: 12000000, accountCount: 3,
        observedAtMs: nowMs, validUntilMs: nowMs + 86400000,
      },
    },
  };
}
