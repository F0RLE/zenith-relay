import { describe, expect, test } from "bun:test";
import type { RuntimeSnapshot } from "../src/features/relay/api/types";
import { isRuntimeRefreshPage, isUsageRefreshPage, startupSnapshotNeedsRetry, usageRefreshDebounceMs, USAGE_EVENT_REFRESH_DEBOUNCE_MS, OVERVIEW_USAGE_EVENT_REFRESH_DEBOUNCE_MS } from "../src/features/relay/state/refreshPolicy";

function snapshot(overrides: Partial<RuntimeSnapshot["gateway"]> = {}, warnings: string[] = []) {
  return {
    runtimeTarget: { kind: "local" },
    gateway: { running: false, candidateCount: 0, routingOrder: [], ...overrides },
    warnings,
  } as RuntimeSnapshot;
}

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

  test("retries snapshots that race local gateway auto-start", () => {
    expect(startupSnapshotNeedsRetry(snapshot({}, ["gateway_configured_but_not_running"]))).toBe(true);
    expect(startupSnapshotNeedsRetry(snapshot({ running: true, candidateCount: 3 }))).toBe(true);
    expect(startupSnapshotNeedsRetry(snapshot({ running: true, candidateCount: 3, routingOrder: [{ candidateId: "account", available: true }] as never }))).toBe(false);
    expect(startupSnapshotNeedsRetry(snapshot({ candidateCount: 3 }))).toBe(false);
    expect(startupSnapshotNeedsRetry({ ...snapshot({}, ["gateway_configured_but_not_running"]), runtimeTarget: { kind: "remote" } } as RuntimeSnapshot)).toBe(false);
  });
});
