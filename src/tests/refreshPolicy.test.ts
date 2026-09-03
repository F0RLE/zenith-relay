import { describe, expect, test } from "bun:test";
import { isRuntimeRefreshPage, isUsageRefreshPage, usageRefreshDebounceMs, USAGE_EVENT_REFRESH_DEBOUNCE_MS, OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS } from "../src/features/relay/state/refreshPolicy";

describe("refresh policy", () => {
  test("limits usage refreshes to pages that render usage data", () => {
    expect(isUsageRefreshPage("overview")).toBe(true);
    expect(isUsageRefreshPage("usage")).toBe(true);
    expect(isUsageRefreshPage("connections")).toBe(false);
    expect(isUsageRefreshPage("settings")).toBe(false);
  });

  test("lets runtime pages keep their independent refresh policy", () => {
    expect(isRuntimeRefreshPage("overview")).toBe(true);
    expect(isRuntimeRefreshPage("pool")).toBe(true);
    expect(isRuntimeRefreshPage("connections")).toBe(true);
    expect(isRuntimeRefreshPage("usage")).toBe(false);
  });

  test("groups overview usage events longer than the live usage table", () => {
    expect(usageRefreshDebounceMs("usage")).toBe(USAGE_EVENT_REFRESH_DEBOUNCE_MS);
    expect(usageRefreshDebounceMs("overview")).toBe(OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS);
    expect(OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS).toBeGreaterThan(USAGE_EVENT_REFRESH_DEBOUNCE_MS);
  });
});
