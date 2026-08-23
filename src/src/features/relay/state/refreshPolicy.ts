import type { PageId } from "../api/types";

export const RUNTIME_REFRESH_INTERVAL_MS = 60_000;
export const ROUTING_REFRESH_INTERVAL_MS = 2_000;
export const RUNTIME_EVENT_REFRESH_DEBOUNCE_MS = 500;
export const USAGE_EVENT_REFRESH_DEBOUNCE_MS = 250;
export const SUCCESS_FEEDBACK_TIMEOUT_MS = 4_000;
export const ERROR_FEEDBACK_TIMEOUT_MS = 60_000;

export function isRuntimeRefreshPage(page: PageId) {
  return page === "overview" || page === "pool" || page === "connections";
}
