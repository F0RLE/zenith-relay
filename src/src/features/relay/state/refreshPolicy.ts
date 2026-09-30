import type { PageId, RuntimeSnapshot } from "../api/types";

export const RUNTIME_REFRESH_INTERVAL_MS = 60_000;
// Runtime activity events update the visible route immediately. This is only
// a low-frequency fallback for remote/older hosts that do not forward events.
export const ROUTING_REFRESH_INTERVAL_MS = 5_000;
export const RUNTIME_EVENT_REFRESH_DEBOUNCE_MS = 500;
// Auto-start happens after the WebView is created. A first snapshot can
// legitimately arrive before the local listener is ready, so give the native
// startup a few bounded chances to publish its live scheduler order.
export const STARTUP_RUNTIME_RETRY_DELAYS_MS = [250, 750, 1_500, 3_000] as const;
// Usage writes can emit both a state and a usage event. Give the writer a
// short settling window so a burst becomes one report refresh.
export const USAGE_EVENT_REFRESH_DEBOUNCE_MS = 500;
// Overview charts are intentionally less eager than the request table. A
// burst of completed requests should settle before the aggregate query runs.
export const OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS = 2_000;

export function isRuntimeRefreshPage(page: PageId) {
  return page === "overview" || page === "pool" || page === "connections";
}

export function isUsageRefreshPage(page: PageId) {
  return page === "overview" || page === "usage";
}

export function usageRefreshDebounceMs(page: PageId) {
  return page === "overview"
    ? OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS
    : USAGE_EVENT_REFRESH_DEBOUNCE_MS;
}

/**
 * Detect the only startup snapshots that should be retried. An intentionally
 * stopped gateway has no warning and must not cause a polling loop. A running
 * gateway with configured candidates but no serialized order indicates that
 * the renderer observed it during the auto-start race.
 */
export function startupSnapshotNeedsRetry(snapshot: RuntimeSnapshot | null) {
  if (!snapshot || snapshot.runtimeTarget.kind !== "local") return false;
  if (snapshot.warnings.includes("gateway_configured_but_not_running")) return true;
  return snapshot.gateway.running
    && snapshot.gateway.candidateCount > 0
    && (snapshot.gateway.routingOrder?.length ?? 0) === 0;
}
