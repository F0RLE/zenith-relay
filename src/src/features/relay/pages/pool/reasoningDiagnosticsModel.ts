import type { ModelSummary } from "../../api/types";
import { normalizeReasoningSelection, supportedReasoningLevels } from "./modelRulesModel";

export type ReasoningDiagnosticStatus = "available" | "read_only" | "policy_disabled" | "unsupported"
  | "toggle" | "budget_tokens" | "adaptive" | "levels_missing" | "unknown"
  | "runtime_levels_missing" | "runtime_levels_differ" | "conflicting_evidence";

/** Explain the snapshot, not provider behavior that the snapshot cannot prove. */
export function modelReasoningDiagnostics(model: ModelSummary) {
  const capabilities = model.catalogCapabilities;
  const normalize = (levels: string[] | undefined): string[] | null => levels === undefined
    ? null : supportedReasoningLevels({ reasoningSupportedLevels: levels });
  const referenceLevels = normalize(model.catalogReasoningEffortLevels ?? capabilities?.reasoningEffortLevels);
  const supportedLevels = normalize(model.reasoningSupportedLevels ?? model.reasoningLevels);
  const allowedLevels = normalize(model.reasoningAllowedLevels);
  const effectiveLevels = normalize(model.reasoningLevels);
  const support = model.catalogReasoning ?? capabilities?.reasoning ?? null;
  const method = model.catalogReasoningMethod ?? capabilities?.reasoningMethod ?? "unknown";
  let status: ReasoningDiagnosticStatus;
  if (supportedLevels?.length && support === false) status = "conflicting_evidence";
  else if (supportedLevels?.length && referenceLevels?.length
    && (supportedLevels.length !== referenceLevels.length || referenceLevels.some((level) => !supportedLevels.includes(level)))) {
    status = "runtime_levels_differ";
  } else if (supportedLevels?.length) {
    const policy = allowedLevels ?? effectiveLevels;
    status = model.reasoningConfigurable === false ? "read_only"
      : model.reasoningConfigurable === true && policy !== null && normalizeReasoningSelection(supportedLevels, policy).length === 0
        ? "policy_disabled" : "available";
  } else if (support === false) status = "unsupported";
  else if (referenceLevels?.length) status = "runtime_levels_missing";
  else if (method === "toggle" || method === "budget_tokens" || method === "adaptive") status = method;
  else if (support === true || method === "effort") status = "levels_missing";
  else status = "unknown";
  return {
    status, support, method, referenceLevels, supportedLevels, allowedLevels, effectiveLevels,
    source: capabilities?.reasoningSource ?? null,
    referenceModel: model.catalogSourceModelId ?? null,
  };
}
