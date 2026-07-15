import type { LocalUsage } from "./api/types";

export type TokenSpeedSample = {
  success: boolean;
  outputTokens: number | null;
  reasoningTokens?: number | null;
  durationMs: number | null;
  ttftMs?: number | null;
  generationDurationMs?: number | null;
};

function measurement(sample: TokenSpeedSample) {
  if (!sample.success || !sample.outputTokens || sample.outputTokens < 0) return null;
  if (!sample.durationMs || sample.durationMs <= 0) return null;
  return { outputTokens: sample.outputTokens, durationMs: sample.durationMs };
}

export function tokenSpeed(sample: TokenSpeedSample) {
  const measured = measurement(sample);
  return measured ? measured.outputTokens * 1_000 / measured.durationMs : null;
}

export function averageTokenSpeed(samples: TokenSpeedSample[]) {
  const totals = samples.reduce((result, sample) => {
    const measured = measurement(sample);
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
    const speed = tokenSpeed({ success: event.success, outputTokens: event.outputTokens, reasoningTokens: event.reasoningTokens, durationMs: event.latencyMs, ttftMs: event.ttftMs });
    if (speed != null) speeds.set(event.accountId, speed);
  }
  return speeds;
}

export function formatTokenSpeed(value: number | null | undefined, locale: string, unit: string) {
  return value == null ? "-" : `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} ${unit}`;
}
