import type { CandidateRuntimeSnapshot, SourceSummary } from "../../api/types";
import { compareStableText } from "../../poolHelpers";
import { compareRoutingOrder } from "../../routingOrder";
import { effectiveSourceProtocolBindings } from "../../sourceProtocolBindings";
import { sourceHost } from "../../sourceUrl";
import { matchesQuery } from "./connectionHelpers";

export type SourceSortColumn = "status" | "name" | "server" | "models";
export type SourceSortKey = "runtime" | SourceSortColumn;
export type SourceSortDirection = "asc" | "desc";

/** Saved source readiness, evaluated independently of pool membership. */
export function sourceStatus(
  source: SourceSummary,
  candidate: CandidateRuntimeSnapshot | undefined,
  nowMs: number,
  enabled = source.enabled,
): { key: string; tone: "ready" | "warning" | "error" | "info" | "disabled" } {
  if (!enabled) return { key: "disabled", tone: "disabled" };
  if (!source.secretAvailable) return { key: "keyMissing", tone: "error" };
  if (source.draining) return { key: "draining", tone: "warning" };
  if (source.lastErrorCode?.trim()) return { key: "error", tone: "error" };
  if (candidate?.halfOpen) return { key: "recovering", tone: "info" };
  if (source.operationalStatus === "quotaWait" || candidate?.nextRetryAtMs != null && candidate.nextRetryAtMs > nowMs) {
    return { key: "retryWait", tone: "warning" };
  }
  if (source.operationalStatus === "unavailable") return { key: "unavailable", tone: "error" };
  return { key: "ready", tone: "ready" };
}

const sourceStatusRank: Record<SourceSummary["operationalStatus"], number> = {
  disabled: 0,
  unavailable: 1,
  quotaWait: 2,
  rotation: 3,
};

function sourceSortValue(source: SourceSummary, key: SourceSortColumn) {
  switch (key) {
    case "status": return sourceStatusRank[source.operationalStatus];
    case "server": return sourceHost(source.baseUrl);
    case "models": return source.models.length;
    case "name": return source.name;
  }
}

export function compareSources(
  left: SourceSummary,
  right: SourceSummary,
  key: SourceSortKey,
  direction: SourceSortDirection,
  runtimePosition: ReadonlyMap<string, number>,
) {
  if (key === "runtime") {
    return compareRoutingOrder(left.id, right.id, runtimePosition)
      || compareStableText(left.name, right.name)
      || compareStableText(left.id, right.id);
  }
  const leftValue = sourceSortValue(left, key);
  const rightValue = sourceSortValue(right, key);
  const primary = typeof leftValue === "number" && typeof rightValue === "number"
    ? leftValue - rightValue
    : compareStableText(String(leftValue), String(rightValue));
  if (primary) return direction === "asc" ? primary : -primary;
  return compareStableText(left.name, right.name) || compareStableText(left.id, right.id);
}

export function filterAndSortSources(
  sources: readonly SourceSummary[],
  query: string,
  key: SourceSortKey,
  direction: SourceSortDirection,
  runtimePosition: ReadonlyMap<string, number>,
) {
  return sources
    .filter((source) => matchesQuery(
      query,
      source.name,
      source.baseUrl,
      effectiveSourceProtocolBindings(source).map((binding) => binding.wireApi),
      source.models,
    ))
    .sort((left, right) => compareSources(left, right, key, direction, runtimePosition));
}
