import type { TFunction } from "i18next";
import { automationDefaultNames } from "../../../../i18n/automationNames";
import type { AccountSummary, RuntimeSnapshot, WakeTask } from "../../api/types";
import { defaultWakeInput } from "../../api/commands";
import { memberModelIsEnabled } from "../../components/poolMemberEditorModel";
import { modelIdKey, orderModelIdsBySnapshot, uniqueModelIds } from "../../modelGroups";

export type AutomationSelectorKind = WakeTask["accountSelector"]["kind"];
export type AutomationTriggerKind = WakeTask["trigger"]["kind"];

export const automationTypes = [
  { triggerKind: "quota_full", nameKey: "automations.defaultName", conditionKey: "automations.primaryRecovery", requiresModel: true },
  { triggerKind: "weekly", nameKey: "automations.weeklyDefaultName", conditionKey: "automations.weeklyReset", requiresModel: false },
] as const;

export function automationType(triggerKind: AutomationTriggerKind) {
  return automationTypes.find((type) => type.triggerKind === triggerKind) ?? automationTypes[0];
}

const defaultNames = new Set<string>(Object.values(automationDefaultNames).flatMap(Object.values));

export function customAutomationName(automationName?: string): string | null {
  return automationName === undefined || defaultNames.has(automationName.trim()) ? null : automationName;
}

export function defaultAutomationName(triggerKind: AutomationTriggerKind, t: TFunction): string {
  return t(automationType(triggerKind).nameKey);
}

export function automationDisplayName(task: Pick<WakeTask, "name" | "trigger">, t: TFunction): string {
  // Older weekly rules persisted the countdown's default name as well.
  return customAutomationName(task.name) ?? defaultAutomationName(task.trigger.kind, t);
}

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
  return orderModelIdsBySnapshot(uniqueModelIds(rawModels), gateway.models ?? []);
}

export function automationTargetModels(
  accounts: readonly AccountSummary[],
  selectorKind: AutomationSelectorKind,
) {
  const modelSets = accounts.map((account) => account.models.filter((model) =>
    memberModelIsEnabled(account.allowedModels, account.excludedModels, model),
  ));
  if (selectorKind !== "account_ids") return modelSets.flat();
  if (modelSets.length <= 1) return modelSets.flat();
  return modelSets[0]!.filter((model) => modelSets.slice(1).every((set) =>
    set.some((candidate) => modelIdKey(candidate) === modelIdKey(model)),
  ));
}

export function availableAutomationModels(
  gateway: RuntimeSnapshot["gateway"],
  targetAccounts: readonly AccountSummary[],
  selectorKind: AutomationSelectorKind,
) {
  const targetModels = automationTargetModels(targetAccounts, selectorKind);
  return automationPoolModels(gateway).filter((model) =>
    targetModels.some((candidate) => modelIdKey(candidate) === modelIdKey(model)),
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
  return availableModels.find((model) => modelIdKey(model) === modelIdKey(requestedModel))
    ?? availableModels[0]
    ?? "";
}

export function automationFormValid(
  automationName: string,
  accountsValid: boolean,
  requiresModel: boolean,
  selectedModel: string,
) {
  return Boolean(automationName.trim() && accountsValid && (!requiresModel || selectedModel));
}

export type AutomationSubmission = {
  operationId: string;
  base: Omit<WakeTask, "id" | "createdAtMs" | "updatedAtMs">;
  remoteInput: WakeTask;
};

export function buildAutomationSubmission(input: {
  task: WakeTask | null;
  automationName: string;
  triggerKind: AutomationTriggerKind;
  selectorKind: AutomationSelectorKind;
  accountIds: readonly string[];
  selectedModel: string;
  nowMs: number;
}): AutomationSubmission {
  const {
    task,
    automationName,
    triggerKind,
    selectorKind,
    accountIds,
    selectedModel,
    nowMs,
  } = input;
  const weeklyReset = triggerKind === "weekly";
  const accountSelector = selectorKind === "account_ids"
    ? { kind: selectorKind, values: [...accountIds] }
    : { kind: "all_eligible" as const };
  const modelPolicy = weeklyReset
    ? { kind: "lightest_supported" as const }
    : { kind: "explicit" as const, value: selectedModel };
  const base = {
    ...defaultWakeInput(automationName),
    enabled: task?.enabled ?? true,
    accountSelector,
    windowKinds: weeklyReset ? ["secondary" as const] : ["primary" as const],
    modelPolicy,
    trigger: { kind: triggerKind },
    executionPolicy: "automatic" as const,
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
