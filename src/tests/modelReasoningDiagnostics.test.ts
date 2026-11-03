import { describe, expect, test } from "bun:test";
import type { ModelSummary } from "../src/features/relay/api/types";
import { modelReasoningDiagnostics } from "../src/features/relay/pages/pool/reasoningDiagnosticsModel";
import { modelSignature, modelShowsReasoningControl, supportedReasoningLevels } from "../src/features/relay/pages/pool/modelRulesModel";

const model = (overrides: Partial<ModelSummary> = {}): ModelSummary => ({
  id: "synthetic-model", enabled: true, memberCount: 1, codexVisible: true,
  codexDisplayName: "Synthetic model", inputMicroUsdPerMillion: null,
  outputMicroUsdPerMillion: null, customPrice: false, ...overrides,
});

describe("reasoning diagnostics", () => {
  test("missing metadata is unknown, not unsupported or disabled by policy", () => {
    const diagnostic = modelReasoningDiagnostics(model({ reasoningSupportedLevels: [], reasoningAllowedLevels: [], reasoningLevels: [] }));
    expect(diagnostic.status).toBe("unknown");
    expect(diagnostic.support).toBeNull();
    expect(diagnostic.source).toBeNull();
    expect(diagnostic.referenceLevels).toBeNull();
  });

  test("reasoning support without an enum never invents levels", () => {
    const diagnostic = modelReasoningDiagnostics(model({ catalogReasoning: true }));
    expect(diagnostic.status).toBe("levels_missing");
    expect(diagnostic.supportedLevels).toBeNull();
    expect(modelReasoningDiagnostics(model({ catalogReasoningMethod: "effort" })).status).toBe("levels_missing");
  });

  test.each(["toggle", "budget_tokens", "adaptive"] as const)("explains %s without calling it unsupported", (method) => {
    expect(modelReasoningDiagnostics(model({ catalogReasoningMethod: method })).status).toBe(method);
  });

  test("explicit unsupported evidence is distinct from an absent enum", () => {
    expect(modelReasoningDiagnostics(model({ catalogReasoning: false, catalogReasoningMethod: "budget_tokens" })).status).toBe("unsupported");
  });

  test("shows a runtime/catalog discrepancy without inventing its cause", () => {
    const current = model({ catalogReasoningEffortLevels: ["low", "high"], reasoningSupportedLevels: [], reasoningLevels: ["stale"] });
    const diagnostic = modelReasoningDiagnostics(current);
    expect(diagnostic.status).toBe("runtime_levels_missing");
    expect(diagnostic.supportedLevels).toEqual([]);
    expect(supportedReasoningLevels(current)).toEqual([]);
    expect(modelShowsReasoningControl(current)).toBeFalse();
  });

  test("distinguishes absent policy data from an explicit empty policy", () => {
    const current = model({ reasoningSupportedLevels: ["low", "high"], reasoningConfigurable: true });
    expect(modelReasoningDiagnostics(current).status).toBe("available");
    expect(modelReasoningDiagnostics(current).allowedLevels).toBeNull();
    expect(modelReasoningDiagnostics({ ...current, reasoningAllowedLevels: [] }).status).toBe("policy_disabled");
    expect(modelReasoningDiagnostics({ ...current, reasoningAllowedLevels: ["stale"] }).status).toBe("policy_disabled");
    expect(modelReasoningDiagnostics({ ...current, reasoningConfigurable: false, reasoningAllowedLevels: [] }).status).toBe("read_only");
  });

  test("keeps supported, allowed and effective levels separate without mutations", () => {
    const current = model({ catalogReasoningEffortLevels: ["low", "very_high"], reasoningSupportedLevels: ["low", "xhigh"], reasoningAllowedLevels: ["xhigh"], reasoningLevels: ["xhigh"] });
    const before = structuredClone(current);
    expect(modelReasoningDiagnostics(current)).toMatchObject({ status: "available", referenceLevels: ["low", "xhigh"], supportedLevels: ["low", "xhigh"], allowedLevels: ["xhigh"], effectiveLevels: ["xhigh"] });
    expect(current).toEqual(before);
  });

  test("flags narrowed runtime lists but not a different order or an operator restriction", () => {
    const current = model({ catalogReasoningEffortLevels: ["low", "high"], reasoningSupportedLevels: ["high"] });
    expect(modelReasoningDiagnostics(current).status).toBe("runtime_levels_differ");
    expect(modelReasoningDiagnostics({ ...current, reasoningSupportedLevels: ["high", "low"], reasoningAllowedLevels: ["high"] }).status).toBe("available");
    expect(modelReasoningDiagnostics({ ...current, catalogReasoning: false }).status).toBe("conflicting_evidence");
  });

  test("reasoning provenance refresh changes the rendered model signature", () => {
    const capabilities = { reasoningSource: "models_dev", reasoningEffortLevels: [], reasoningModes: [], serviceTiers: [], inputModalities: [], outputModalities: [] };
    const current = model({ catalogCapabilities: capabilities, catalogSourceModelId: "provider/synthetic" });
    expect(modelReasoningDiagnostics(current).source).toBe("models_dev");
    expect(modelSignature([current])).not.toBe(modelSignature([{ ...current, catalogCapabilities: { ...capabilities, reasoningSource: "litellm" } }]));
  });
});
