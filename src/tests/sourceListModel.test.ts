import { describe, expect, test } from "bun:test";
import type { SourceSummary } from "../src/features/relay/api/types";
import { filterAndSortSources } from "../src/features/relay/pages/connections/sourceListModel";

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
});
