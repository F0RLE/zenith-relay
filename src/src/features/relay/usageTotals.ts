import type { UsageTotals } from "./api/types";
import { measureTokenSpeed } from "./usageSpeed";

/** Canonical input for every UI aggregation of request telemetry. */
export type UsageTotalsSample = {
  success: boolean;
  latencyMs: number;
  ttftMs: number | null;
  generationMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalent?: UsageTotals["apiEquivalent"] | null;
};

export function emptyUsageTotals(): UsageTotals {
  return {
    requests: 0,
    successfulRequests: 0,
    latencyMs: 0,
    ttftMs: 0,
    ttftSamples: 0,
    generationMs: 0,
    generationSamples: 0,
    generationOutputTokens: 0,
    inputTokens: 0,
    cachedInputTokens: 0,
    cachedInputSamples: 0,
    cacheWriteInputTokens: 0,
    cacheWriteInputSamples: 0,
    reasoningTokens: 0,
    outputTokens: 0,
    totalTokens: 0,
    speedOutputTokens: 0,
    speedDurationMs: 0,
    apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 },
  };
}

/** Aggregates local and remote telemetry with one accounting policy. */
export function totalsFromUsageSamples(samples: Iterable<UsageTotalsSample>): UsageTotals {
  const totals = emptyUsageTotals();
  for (const sample of samples) {
    const outputTokens = sample.success ? Math.max(0, sample.outputTokens ?? 0) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(sample.success);
    totals.latencyMs += sample.latencyMs;
    if (sample.ttftMs != null) {
      totals.ttftMs += sample.ttftMs;
      totals.ttftSamples += 1;
    }
    const generation = measureTokenSpeed({
      success: sample.success,
      outputTokens: sample.outputTokens,
      reasoningTokens: sample.reasoningTokens,
      durationMs: sample.generationMs,
    });
    if (generation) {
      totals.generationMs += generation.durationMs;
      totals.generationSamples += 1;
      totals.generationOutputTokens += generation.outputTokens;
    }
    totals.inputTokens += sample.inputTokens ?? 0;
    if (sample.cachedInputTokens != null) {
      totals.cachedInputTokens += sample.cachedInputTokens;
      totals.cachedInputSamples += 1;
    }
    if (sample.cacheWriteInputTokens != null) {
      totals.cacheWriteInputTokens = (totals.cacheWriteInputTokens ?? 0) + sample.cacheWriteInputTokens;
      totals.cacheWriteInputSamples = (totals.cacheWriteInputSamples ?? 0) + 1;
    }
    totals.reasoningTokens += sample.reasoningTokens ?? 0;
    totals.outputTokens += sample.outputTokens ?? 0;
    totals.totalTokens += sample.totalTokens ?? 0;
    if (outputTokens > 0 && sample.latencyMs > 0) {
      totals.speedOutputTokens += outputTokens;
      totals.speedDurationMs += sample.latencyMs;
    }
    if (sample.apiEquivalent) {
      totals.apiEquivalent.microUsd += sample.apiEquivalent.microUsd;
      totals.apiEquivalent.pricedTokens += sample.apiEquivalent.pricedTokens;
      totals.apiEquivalent.unpricedTokens += sample.apiEquivalent.unpricedTokens;
    } else {
      totals.apiEquivalent.unpricedTokens += sample.totalTokens ?? 0;
    }
  }
  return totals;
}

export function formatCompactNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, {
    notation: Math.abs(value) >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

export function formatFullNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value);
}
