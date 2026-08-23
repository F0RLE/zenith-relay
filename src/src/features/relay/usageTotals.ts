import type { UsageTotals } from "./api/types";

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

export function formatCompactNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, {
    notation: Math.abs(value) >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

export function formatFullNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value);
}
