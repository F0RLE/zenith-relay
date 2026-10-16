import type { UsageTotals } from "./api/types";
import { getNumberFormatter } from "./numberFormatting";
import { isReasonableTokenSpeed, measureTokenSpeed } from "./usageSpeed";

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
  const usageTotals = emptyUsageTotals();
  for (const sample of samples) {
    const outputTokens = sample.success ? Math.max(0, sample.outputTokens ?? 0) : 0;
    usageTotals.requests += 1;
    usageTotals.successfulRequests += Number(sample.success);
    usageTotals.latencyMs += sample.latencyMs;
    if (sample.ttftMs != null) {
      usageTotals.ttftMs += sample.ttftMs;
      usageTotals.ttftSamples += 1;
    }
    const generation = measureTokenSpeed({
      success: sample.success,
      outputTokens: sample.outputTokens,
      reasoningTokens: sample.reasoningTokens,
      durationMs: sample.generationMs,
    });
    if (generation) {
      usageTotals.generationMs += generation.durationMs;
      usageTotals.generationSamples += 1;
      usageTotals.generationOutputTokens += generation.outputTokens;
    }
    usageTotals.inputTokens += sample.inputTokens ?? 0;
    if (sample.cachedInputTokens != null) {
      usageTotals.cachedInputTokens += sample.cachedInputTokens;
      usageTotals.cachedInputSamples += 1;
    }
    if (sample.cacheWriteInputTokens != null) {
      usageTotals.cacheWriteInputTokens = (usageTotals.cacheWriteInputTokens ?? 0) + sample.cacheWriteInputTokens;
      usageTotals.cacheWriteInputSamples = (usageTotals.cacheWriteInputSamples ?? 0) + 1;
    }
    usageTotals.reasoningTokens += sample.reasoningTokens ?? 0;
    usageTotals.outputTokens += sample.outputTokens ?? 0;
    usageTotals.totalTokens += sample.totalTokens ?? 0;
    if (isReasonableTokenSpeed(outputTokens, sample.latencyMs)) {
      usageTotals.speedOutputTokens += outputTokens;
      usageTotals.speedDurationMs += sample.latencyMs;
    }
    if (sample.apiEquivalent) {
      usageTotals.apiEquivalent.microUsd += sample.apiEquivalent.microUsd;
      usageTotals.apiEquivalent.pricedTokens += sample.apiEquivalent.pricedTokens;
      usageTotals.apiEquivalent.unpricedTokens += sample.apiEquivalent.unpricedTokens;
    } else {
      usageTotals.apiEquivalent.unpricedTokens += sample.totalTokens ?? 0;
    }
  }
  return usageTotals;
}

export function formatCompactNumber(numericValue: number, locale: string) {
  const notation = Math.abs(numericValue) >= 1_000 ? "compact" : "standard";
  return getNumberFormatter(locale, {
    notation,
    maximumFractionDigits: 1,
  }).format(numericValue);
}

export function formatFullNumber(numericValue: number, locale: string) {
  return getNumberFormatter(locale, { maximumFractionDigits: 0 }).format(numericValue);
}
