import type { SourceSummary } from "../../api/types";
import { compareStableText } from "../../poolHelpers";
import { compareRoutingOrder } from "../../routingOrder";
import { effectiveSourceProtocolBindings } from "../../sourceProtocolBindings";
import { sourceHost } from "../../sourceUrl";
import { matchesQuery } from "./connectionHelpers";

export type SourceSortColumn = "status" | "name" | "server" | "models";
export type SourceSortKey = "runtime" | SourceSortColumn;
export type SourceSortDirection = "asc" | "desc";

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
