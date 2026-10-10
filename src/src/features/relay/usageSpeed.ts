import type { LocalUsage } from "./api/types";
import { formatNumber } from "./numberFormatting";

export type TokenSpeedSample = {
  success: boolean;
  outputTokens: number | null;
  reasoningTokens?: number | null;
  durationMs: number | null;
};

export type TokenSpeedMeasurement = {
  outputTokens: number;
  durationMs: number;
};

// A tiny generation window usually means the upstream buffered the response
// and released its first chunk near completion. Do not present that artifact
// as a meaningful throughput measurement.
export const MAX_REASONABLE_TOKEN_SPEED = 1_000;

export function isReasonableTokenSpeed(outputTokens: number, durationMs: number) {
  return outputTokens > 0 && durationMs > 0 && outputTokens * 1_000 / durationMs <= MAX_REASONABLE_TOKEN_SPEED;
}

/** Throughput already aggregated from observed output and duration. */
export function observedTokensPerSecond(outputTokens: number, durationMs: number): number | null {
  if (outputTokens <= 0 || durationMs <= 0) return null;
  return outputTokens * 1_000 / durationMs;
}

export function measureTokenSpeed(sample: TokenSpeedSample): TokenSpeedMeasurement | null {
  if (!sample.success || sample.outputTokens == null || sample.outputTokens < 0) return null;
  if (!sample.durationMs || sample.durationMs <= 0) return null;
  const reasoningTokens = Math.min(sample.outputTokens, Math.max(0, sample.reasoningTokens ?? 0));
  const outputTokens = Math.max(0, sample.outputTokens - reasoningTokens - 1);
  if (outputTokens <= 0) return null;
  return isReasonableTokenSpeed(outputTokens, sample.durationMs) ? { outputTokens, durationMs: sample.durationMs } : null;
}

export function tokenSpeed(sample: TokenSpeedSample) {
  const measured = measureTokenSpeed(sample);
  return measured ? observedTokensPerSecond(measured.outputTokens, measured.durationMs) : null;
}

export function averageTokenSpeed(samples: TokenSpeedSample[]) {
  const totals = samples.reduce((speedTotals, sample) => {
    const measured = measureTokenSpeed(sample);
    if (measured) {
      speedTotals.outputTokens += measured.outputTokens;
      speedTotals.durationMs += measured.durationMs;
    }
    return speedTotals;
  }, { outputTokens: 0, durationMs: 0 });
  return observedTokensPerSecond(totals.outputTokens, totals.durationMs);
}

export function latestLocalAccountSpeeds(events: LocalUsage[]) {
  const speeds = new Map<string, number>();
  for (const event of [...events].sort((left, right) => Date.parse(right.createdAt) - Date.parse(left.createdAt))) {
    if (!event.accountId || speeds.has(event.accountId)) continue;
    const speed = tokenSpeed({ success: event.success, outputTokens: event.outputTokens, reasoningTokens: event.reasoningTokens, durationMs: event.generationMs });
    if (speed != null) speeds.set(event.accountId, speed);
  }
  return speeds;
}

export function formatTokenSpeed(tokensPerSecond: number | null | undefined, locale: string, unit: string) {
  return tokensPerSecond == null ? "-" : `${formatNumber(tokensPerSecond, locale, { maximumFractionDigits: 1 })} ${unit}`;
}
