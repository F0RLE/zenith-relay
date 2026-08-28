import { describe, expect, test } from "bun:test";
import type { AccountSummary, SourceSummary } from "../src/features/relay/api/types";
import {
  modelSelectionForMember,
  modelSelectionPayload,
  moveSourceBy,
  moveSourceOrder,
  sourcePrioritiesForOrder,
} from "../src/features/relay/components/poolMemberEditorModel";

const source = (overrides: Partial<SourceSummary> = {}): SourceSummary => ({
  id: "source",
  name: "Source",
  enabled: true,
  inPool: true,
  draining: false,
  operationalStatus: "rotation",
  baseUrl: "https://example.test/v1",
  wireApi: "responses",
  models: ["gpt-5.4"],
  allowedModels: [],
  excludedModels: [],
  priority: 1,
  weight: 1,
  recoveryDelaySeconds: 0,
  apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
  secretAvailable: true,
  lastErrorCode: null,
  ...overrides,
});

const account = (overrides: Partial<AccountSummary> = {}): AccountSummary => ({
  id: "account",
  label: "Account",
  identityHint: "Account",
  enabled: true,
  inPool: true,
  draining: false,
  authState: { state: "ready" },
  health: "ready",
  operationalStatus: "rotation",
  models: ["gpt-5.4", "gpt-5.4-mini"],
  allowedModels: ["gpt-5.4"],
  excludedModels: [],
  priority: 1,
  weight: 1,
  apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
  subscription: { planType: null, activeUntilMs: null, status: "active", updatedAtMs: null },
  quota: {},
  quotaRefreshStatus: "updated",
  secretAvailable: true,
  lastErrorCode: null,
  ...overrides,
});

describe("pool member editor model", () => {
  test("merges model sources case-insensitively and keeps explicit exclusions", () => {
    expect(modelSelectionForMember({ ...source({
      models: ["GPT-5.4", "custom"],
      allowedModels: ["gpt-5.4"],
      excludedModels: ["CUSTOM"],
      modelPriceOverrides: { "model-x": { inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2 } },
    }), kind: "source" })).toEqual({
      modelIds: ["model-x", "GPT-5.4", "custom"],
      enabledModels: ["GPT-5.4"],
    });
  });

  test("serializes a full selection as empty allow/deny lists", () => {
    expect(modelSelectionPayload(["A", "b"], ["a", "B"])).toEqual({ allowedModels: [], excludedModels: [] });
    expect(modelSelectionPayload(["A", "b"], ["a"])).toEqual({ allowedModels: ["A"], excludedModels: ["b"] });
  });

  test("moves sources before or after a target without mutating the original order", () => {
    const current = ["a", "b", "c"];
    expect(moveSourceOrder(current, "a", "c")).toEqual(["b", "a", "c"]);
    expect(moveSourceOrder(current, "a", "c", true)).toEqual(["b", "c", "a"]);
    expect(moveSourceBy(current, "b", -1)).toEqual(["b", "a", "c"]);
    expect(moveSourceBy(current, "b", 1)).toEqual(["a", "c", "b"]);
    expect(current).toEqual(["a", "b", "c"]);
  });

  test("builds priorities from the selected role and visual order", () => {
    expect(sourcePrioritiesForOrder(["a", "b"], "primary")).toEqual({ a: 1_000_002, b: 1_000_001 });
    expect(sourcePrioritiesForOrder(["a", "b"], "reserve")).toEqual({ a: -1_000_000, b: -1_000_001 });
    expect(modelSelectionForMember({ ...account(), kind: "account" }).enabledModels).toEqual(["gpt-5.4"]);
  });
});
