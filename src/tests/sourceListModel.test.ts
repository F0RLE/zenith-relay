import { describe, expect, test } from "bun:test";
import type { CandidateRuntimeSnapshot, SourceSummary } from "../src/features/relay/api/types";
import { filterAndSortSources, sourceStatus } from "../src/features/relay/pages/connections/sourceListModel";

const source = (id: string, overrides: Partial<SourceSummary> = {}): SourceSummary => ({
  id,
  name: id,
  enabled: true,
  inPool: true,
  draining: false,
  operationalStatus: "rotation",
  baseUrl: `https://${id}.example`,
  wireApi: "responses",
  models: ["gpt"],
  allowedModels: [],
  excludedModels: [],
  priority: 1,
  weight: 1,
  recoveryDelaySeconds: 0,
  apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 },
  secretAvailable: true,
  lastErrorCode: null,
  ...overrides,
});

describe("source list model", () => {
  test("follows runtime order and uses the name only as a tie-break", () => {
    const sources = [source("b"), source("a"), source("c")];
    const order = new Map([["c", 0], ["a", 1]]);
    expect(filterAndSortSources(sources, "", "runtime", "asc", order).map((item) => item.id)).toEqual(["c", "a", "b"]);
  });

  test("sorts status by availability and reverses a chosen column", () => {
    const sources = [
      source("ready", { operationalStatus: "rotation", models: ["a"] }),
      source("down", { operationalStatus: "unavailable", models: ["a", "b", "c"] }),
      source("off", { operationalStatus: "disabled", models: ["a", "b"] }),
    ];
    expect(filterAndSortSources(sources, "", "status", "asc", new Map()).map((item) => item.id)).toEqual(["off", "down", "ready"]);
    expect(filterAndSortSources(sources, "", "models", "desc", new Map()).map((item) => item.id)).toEqual(["down", "off", "ready"]);
  });

  test("filters by name, host, and model without changing the source list", () => {
    const sources = [
      source("alpha", { baseUrl: "https://api.example", models: ["claude"] }),
      source("beta", { name: "Backup", models: ["gpt"] }),
    ];
    expect(filterAndSortSources(sources, "backup", "name", "asc", new Map()).map((item) => item.id)).toEqual(["beta"]);
    expect(filterAndSortSources(sources, "claude", "name", "asc", new Map()).map((item) => item.id)).toEqual(["alpha"]);
    expect(sources.map((item) => item.id)).toEqual(["alpha", "beta"]);
  });

  test("source readiness does not claim account rotation or depend on pool membership", () => {
    for (const inPool of [true, false]) {
      expect(sourceStatus(source("api", { inPool }), undefined, 100)).toEqual({ key: "ready", tone: "ready" });
    }
  });

  test("source-owned failures and draining take precedence over readiness", () => {
    const cases: Array<[Partial<SourceSummary>, string, string]> = [
      [{ enabled: false, lastErrorCode: "synthetic_error" }, "disabled", "disabled"],
      [{ secretAvailable: false }, "keyMissing", "error"],
      [{ draining: true }, "draining", "warning"],
      [{ lastErrorCode: "synthetic_error" }, "error", "error"],
      [{ operationalStatus: "unavailable" }, "unavailable", "error"],
      [{ operationalStatus: "quotaWait" }, "retryWait", "warning"],
    ];
    for (const [overrides, key, tone] of cases) {
      expect(sourceStatus(source("api", overrides), undefined, 100)).toEqual({ key, tone });
    }
    expect(sourceStatus(source("api"), undefined, 100, false).key).toBe("disabled");
  });

  test("source status retains runtime recovery and bounded retry states", () => {
    const candidate: CandidateRuntimeSnapshot = {
      candidateId: "api", kind: "api_source", available: false, inFlight: 0,
      lastUsedAtMs: null, nextRetryAtMs: 200, halfOpen: false, dispatches: 0,
    };
    expect(sourceStatus(source("api"), candidate, 100)).toEqual({ key: "retryWait", tone: "warning" });
    expect(sourceStatus(source("api"), candidate, 200)).toEqual({ key: "ready", tone: "ready" });
    expect(sourceStatus(source("api"), { ...candidate, halfOpen: true }, 100)).toEqual({ key: "recovering", tone: "info" });
  });
});
