import type { WeeklyCapacityModel, WeeklyCapacityReport, WeeklyCapacitySeries } from "../../api/types/weeklyCapacity";
import { DAY_MS } from "./overviewAnalyticsModel";

export function capacityModelOptions(models: WeeklyCapacityModel[]) {
  return [...new Set(models.map(({ model }) => model))].sort()
    .map((model) => ({ value: model, label: model }));
}

export function capacityCurrent(group: WeeklyCapacitySeries | undefined, nowMs: number) {
  const point = group?.current;
  if (!point || point.observedAtMs > nowMs || point.validUntilMs <= nowMs || nowMs - point.observedAtMs >= 3 * DAY_MS) return null;
  return { point, stale: Boolean(group?.stale) || nowMs - point.sampleStartMs >= DAY_MS };
}

export function nextCapacityExpiry(report: WeeklyCapacityReport, nowMs: number) {
  const deadlines = [...report.summaries, ...(report.models ?? []), ...report.groups].flatMap((group) => group.current
    ? [group.current.sampleStartMs + DAY_MS, group.current.validUntilMs, group.current.observedAtMs + 3 * DAY_MS]
    : []).filter((at) => at > nowMs);
  return deadlines.length ? Math.min(...deadlines) : null;
}

type Entry = { revision: number; report: WeeklyCapacityReport; expiresAt: number };
const cache = new Map<string, Entry>();
const pending = new Map<string, Promise<WeeklyCapacityReport>>();
const latest = new Map<string, Promise<WeeklyCapacityReport>>();

export function cachedCapacity(key: string, revision: number) {
  const entry = cache.get(key);
  // A cleared history/new revision must never flash an older curve.
  return entry?.revision === revision ? entry.report : null;
}

export function loadCapacity(key: string, revision: number, load: () => Promise<WeeklyCapacityReport>, nowMs = Date.now()) {
  const entry = cache.get(key);
  if (entry?.revision === revision && entry.expiresAt > nowMs) return Promise.resolve(entry.report);
  const requestKey = JSON.stringify([key, revision]);
  const existing = pending.get(requestKey);
  if (existing) return existing;
  const request = load().then((report) => {
    if (!report || !Array.isArray(report.summaries) || !Array.isArray(report.groups) || !Array.isArray(report.events)) throw new Error("Invalid weekly capacity report");
    if (report.models !== undefined && !Array.isArray(report.models)) throw new Error("Invalid weekly capacity models");
    // Native responses may finish out of order after a reset or history clear.
    if (latest.get(key) === request) {
      cache.delete(key);
      cache.set(key, { revision, report, expiresAt: Math.min(nowMs + 5 * 60_000, nextCapacityExpiry(report, nowMs) ?? Infinity) });
      while (cache.size > 8) cache.delete(cache.keys().next().value!);
    }
    return report;
  }).finally(() => {
    pending.delete(requestKey);
    if (latest.get(key) === request) latest.delete(key);
  });
  pending.set(requestKey, request);
  latest.set(key, request);
  return request;
}
