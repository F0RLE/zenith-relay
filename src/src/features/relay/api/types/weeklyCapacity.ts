/** Observed full weekly-window capacity, never an available dollar balance. */
export type WeeklyCapacityPoint = {
  atMs: number;
  observedAtMs: number;
  sampleStartMs: number;
  validUntilMs: number;
  medianMicroUsd: number;
  lowMicroUsd: number;
  highMicroUsd: number;
  accountCount: number;
  consumedBasisPoints: number;
};

export type WeeklyCapacityEvent = {
  atMs: number;
  plan: string;
  kind: "reset" | "plan_changed" | "pricing_changed" | "discontinuity";
  count: number;
};

export type WeeklyCapacitySeries = {
  points: WeeklyCapacityPoint[];
  current: WeeklyCapacityPoint | null;
  stale: boolean;
};

export type WeeklyCapacityGroup = WeeklyCapacitySeries & {
  id: string;
  plan: string;
  workload: string;
  cacheBand: number;
  outputBand: number;
  pricingRevision: string;
};

/** All workloads of one plan and price revision, deduplicated by the native ledger. */
export type WeeklyCapacitySummary = WeeklyCapacitySeries & {
  plan: string;
  pricingRevision: string;
};

export type WeeklyCapacityReport = {
  generatedAtMs: number;
  summaries: WeeklyCapacitySummary[];
  groups: WeeklyCapacityGroup[];
  events: WeeklyCapacityEvent[];
};
