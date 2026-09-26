import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot, SourceSummary } from "../src/features/relay/api/types";
import { groupModels, memberModelCatalog } from "../src/features/relay/modelGroups";
import {
  modelSelectionForMember,
  modelSelectionPayload,
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
  test("complete metadata groups excluded models without guessing from IDs", () => {
    const catalog = memberModelCatalog({
      models: [{ id: "older", catalogProvider: "openai" }],
      modelCatalog: { NEWER: { catalogProvider: "openai" }, unrelated: { catalogProvider: "anthropic" } },
    } as RuntimeSnapshot["gateway"]);
    const groups = groupModels(["newer", "older", "unrelated", "gpt-custom-alias"], {
      metadata: (model) => catalog.get(model),
    });
    expect(groups.map((group) => [group.provider, group.items])).toEqual([
      ["openai", ["newer", "older"]],
      ["anthropic", ["unrelated"]],
      ["other", ["gpt-custom-alias"]],
    ]);
    expect(memberModelCatalog(undefined).size).toBe(0);
  });

  test("merges model sources case-insensitively and keeps explicit exclusions", () => {
    expect(modelSelectionForMember({ ...source({
      models: ["GPT-5.4", "custom"],
      allowedModels: ["gpt-5.4"],
      excludedModels: ["CUSTOM"],
      modelPriceOverrides: { "model-x": { inputMicroUsdPerMillion: 1, outputMicroUsdPerMillion: 2 } },
    }), kind: "source" })).toEqual({
      modelIds: ["GPT-5.4", "custom", "model-x"],
      enabledModels: ["GPT-5.4"],
    });
  });

  test("serializes a full selection as empty allow/deny lists", () => {
    expect(modelSelectionPayload(["A", "b"], ["a", "B"])).toEqual({ allowedModels: [], excludedModels: [] });
    expect(modelSelectionPayload(["A", "b"], ["a"])).toEqual({ allowedModels: ["A"], excludedModels: ["b"] });
  });

  test("account rules cannot reorder the complete backend inventory", () => {
    const member = { ...account({
      models: ["newer", "older", "alias"],
      allowedModels: ["alias", "older"],
      excludedModels: ["NEWER", "retired"],
    }), kind: "account" as const };
    const initial = modelSelectionForMember(member);
    expect(initial.modelIds).toEqual(["newer", "older", "alias", "retired"]);
    expect(initial.enabledModels).toEqual(["older", "alias"]);
    const changed = modelSelectionPayload(initial.modelIds, ["newer", "alias"]);
    expect(modelSelectionForMember({ ...member, ...changed })).toEqual({
      modelIds: initial.modelIds,
      enabledModels: ["newer", "alias"],
    });
  });

});
