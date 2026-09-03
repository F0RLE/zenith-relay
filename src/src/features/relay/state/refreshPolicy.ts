import type { PageId } from "../api/types";

export const RUNTIME_REFRESH_INTERVAL_MS = 60_000;
// Runtime activity events update the visible route immediately. This is only
// a low-frequency fallback for remote/older hosts that do not forward events.
export const ROUTING_REFRESH_INTERVAL_MS = 5_000;
export const RUNTIME_EVENT_REFRESH_DEBOUNCE_MS = 500;
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
