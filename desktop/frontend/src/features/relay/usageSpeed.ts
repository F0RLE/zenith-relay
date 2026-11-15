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

export function formatTokenSpeed(tokensPerSecond: number | null | undefined, locale: string, unit: string) {
  return tokensPerSecond == null ? "-" : `${formatNumber(tokensPerSecond, locale, { maximumFractionDigits: 1 })} ${unit}`;
}
