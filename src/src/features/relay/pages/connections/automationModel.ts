import type { AccountSummary, RuntimeSnapshot, WakeTask } from "../../api/types";
import { defaultWakeInput } from "../../api/commands";
import { sortModelIdsForLauncher } from "../../modelGroups";

export type AutomationSelectorKind = WakeTask["accountSelector"]["kind"];
export type AutomationTriggerKind = WakeTask["trigger"]["kind"];

export function eligibleAutomationAccounts(accounts: readonly AccountSummary[]) {
  return accounts.filter((account) => account.inPool && account.enabled && !account.draining);
}

export function selectedAutomationAccounts(accounts: readonly AccountSummary[], accountIds: readonly string[]) {
  const selected = new Set(accountIds);
  return accounts.filter((account) => selected.has(account.id));
}

export function automationPoolModels(gateway: RuntimeSnapshot["gateway"]) {
  const rawModels = gateway.visibleModelIds.length
    ? gateway.visibleModelIds
    : (gateway.models ?? []).filter((model) => model.enabled).map((model) => model.id);
  const uniqueModels = rawModels.filter((model, index) =>
    rawModels.findIndex((candidate) => candidate.toLowerCase() === model.toLowerCase()) === index,
  );
  return sortModelIdsForLauncher(uniqueModels);
}

export function automationTargetModels(
  accounts: readonly AccountSummary[],
  selectorKind: AutomationSelectorKind,
) {
  const modelSets = accounts.map((account) => account.models.filter((model) =>
    (account.allowedModels.length === 0 || account.allowedModels.some((allowed) => allowed.toLowerCase() === model.toLowerCase()))
    && !account.excludedModels.some((excluded) => excluded.toLowerCase() === model.toLowerCase()),
  ));
  if (selectorKind !== "account_ids") return modelSets.flat();
  if (modelSets.length <= 1) return modelSets.flat();
  return modelSets[0]!.filter((model) => modelSets.slice(1).every((set) =>
    set.some((candidate) => candidate.toLowerCase() === model.toLowerCase()),
  ));
}

export function availableAutomationModels(
  gateway: RuntimeSnapshot["gateway"],
  targetAccounts: readonly AccountSummary[],
  selectorKind: AutomationSelectorKind,
) {
  const targetModels = automationTargetModels(targetAccounts, selectorKind);
  return automationPoolModels(gateway).filter((model) =>
    targetModels.some((candidate) => candidate.toLowerCase() === model.toLowerCase()),
  );
}

export function automationAccountSelectionValid(
  selectorKind: AutomationSelectorKind,
  poolAccounts: readonly AccountSummary[],
  accountIds: readonly string[],
  selectedAccounts: readonly AccountSummary[],
) {
  if (selectorKind === "all_eligible") return poolAccounts.length > 0;
  return selectorKind === "account_ids" && accountIds.length > 0 && selectedAccounts.length === accountIds.length;
}

export function resolveAutomationModel(availableModels: readonly string[], requestedModel: string) {
  return availableModels.find((model) => model.toLowerCase() === requestedModel.trim().toLowerCase())
    ?? availableModels[0]
    ?? "";
}

export function automationFormValid(name: string, accountsValid: boolean, weeklyReset: boolean, selectedModel: string) {
  return Boolean(name.trim() && accountsValid && (weeklyReset || selectedModel));
}

export type AutomationSubmission = {
  operationId: string;
  base: Omit<WakeTask, "id" | "createdAtMs" | "updatedAtMs">;
  remoteInput: WakeTask;
};

export function buildAutomationSubmission(input: {
  task: WakeTask | null;
  name: string;
  executionPolicy: WakeTask["executionPolicy"];
  triggerKind: AutomationTriggerKind;
  selectorKind: AutomationSelectorKind;
  accountIds: readonly string[];
  selectedModel: string;
  nowMs: number;
}): AutomationSubmission {
  const { task, name, executionPolicy, triggerKind, selectorKind, accountIds, selectedModel, nowMs } = input;
  const weeklyReset = triggerKind === "weekly";
  const accountSelector = selectorKind === "account_ids"
    ? { kind: selectorKind, values: [...accountIds] }
    : { kind: "all_eligible" as const };
  const modelPolicy = weeklyReset
    ? { kind: "lightest_supported" as const }
    : { kind: "explicit" as const, value: selectedModel };
  const base = {
    ...defaultWakeInput(name),
    enabled: task?.enabled ?? true,
    accountSelector,
    windowKinds: weeklyReset ? ["secondary" as const] : ["primary" as const],
    modelPolicy,
    trigger: { kind: triggerKind },
    executionPolicy: weeklyReset ? "automatic" as const : executionPolicy,
    jitterSeconds: task?.jitterSeconds ?? 0,
    maxAttemptsPerCycle: task?.maxAttemptsPerCycle ?? 1,
  };
  const remoteInput = task
    ? { ...task, ...base, updatedAtMs: nowMs }
    : { ...base, id: "", fallbackSchedule: null, createdAtMs: nowMs, updatedAtMs: nowMs };
  return {
    operationId: task ? `automation-update-${task.id}` : "automation-create",
    base,
    remoteInput,
  };
}
