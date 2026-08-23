import type { LocalUsage } from "./api/types";

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

export function measureTokenSpeed(sample: TokenSpeedSample): TokenSpeedMeasurement | null {
  if (!sample.success || sample.outputTokens == null || sample.outputTokens < 0) return null;
  if (!sample.durationMs || sample.durationMs <= 0) return null;
  const reasoningTokens = Math.min(sample.outputTokens, Math.max(0, sample.reasoningTokens ?? 0));
  const outputTokens = Math.max(0, sample.outputTokens - reasoningTokens - 1);
  return outputTokens > 0 ? { outputTokens, durationMs: sample.durationMs } : null;
}

export function tokenSpeed(sample: TokenSpeedSample) {
  const measured = measureTokenSpeed(sample);
  return measured ? measured.outputTokens * 1_000 / measured.durationMs : null;
}

export function averageTokenSpeed(samples: TokenSpeedSample[]) {
  const totals = samples.reduce((result, sample) => {
    const measured = measureTokenSpeed(sample);
    if (measured) {
      result.outputTokens += measured.outputTokens;
      result.durationMs += measured.durationMs;
    }
    return result;
  }, { outputTokens: 0, durationMs: 0 });
  return totals.durationMs ? totals.outputTokens * 1_000 / totals.durationMs : null;
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

export function formatTokenSpeed(value: number | null | undefined, locale: string, unit: string) {
  return value == null ? "-" : `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} ${unit}`;
}
