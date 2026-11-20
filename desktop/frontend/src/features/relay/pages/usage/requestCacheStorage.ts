import { formatNumber } from "../../numberFormatting";
import type { ToolUseDiagnostics } from "../../api/types";

const units = {
  ms: ["millisecond", 1], s: ["second", 1_000], m: ["minute", 60_000],
  h: ["hour", 3_600_000], d: ["day", 86_400_000],
} as const;

function duration(raw: string) {
  const match = /^([1-9]\d{0,9})(ms|s|m|h|d)$/.exec(raw.trim());
  if (!match || Number(match[1]) > 0xffff_ffff) return null;
  const [unit, multiplier] = units[match[2] as keyof typeof units];
  const value = Number(match[1]);
  return { unit, value, ms: value * multiplier };
}

export function durationLabel(window: NonNullable<ReturnType<typeof duration>>, locale: string) {
  return formatNumber(window.value, locale, { style: "unit", unit: window.unit, unitDisplay: "short" });
}

/** All evidence belongs to this send; a policy is not proof that any prefix was written. */
export function requestCacheStorage(usage: ToolUseDiagnostics["providerUsage"], ttl: string | null) {
  const writes = Object.entries(usage?.cacheWriteTokensByTtl ?? {}).flatMap(([key, count]) => {
    const window = duration(key);
    return window && Number.isSafeInteger(count) && count > 0 ? [{ window, count, key }] : [];
  }).sort((a, b) => a.window.ms - b.window.ms);
  const mode = usage?.cacheMode === "explicit" || usage?.cacheMode === "implicit" ? usage.cacheMode : null;
  const retention = usage?.cacheRetention === "in_memory" ? "in_memory" as const : duration(usage?.cacheRetention ?? "");
  // Older records do not distinguish a returned policy from measured writes.
  const reported = writes.length ? [] : (ttl ?? "").split(",").flatMap((token) => {
    const window = duration(token);
    return window && (retention === "in_memory" || retention?.ms !== window.ms) ? [window] : [];
  });
  return { mode, retention, reported, writes };
}
