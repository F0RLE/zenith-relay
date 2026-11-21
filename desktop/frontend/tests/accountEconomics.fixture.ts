import type { QuotaWindowUsage } from "../src/features/relay/api/types";

export function calibratedWeek(now = Date.now(), overrides: Partial<QuotaWindowUsage> = {}): QuotaWindowUsage {
  return {
    ledgerKey: "synthetic-ledger", kind: "secondary", windowStartMs: now - 86_400_000,
    resetAtMs: now + 6 * 86_400_000, observedAtMs: now, windowMinutes: 10_080,
    availableBasisPoints: 5_800,
    calibration: {
      profileId: "synthetic-shared-profile", workload: "synthetic-model · default",
      cacheBand: 75, outputBand: 10, medianMicroUsd: 10_000_000,
      lowMicroUsd: 8_000_000, highMicroUsd: 12_000_000, accountCount: 3,
      observedAtMs: now, validUntilMs: now + 86_400_000,
    }, ...overrides,
  };
}
