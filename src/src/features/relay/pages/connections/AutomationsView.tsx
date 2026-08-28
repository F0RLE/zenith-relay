import { useState } from "react";
import { Pencil, Play, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { WakeTask } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { NoResults, matchesQuery } from "./connectionHelpers";
import {
  automationAccountSelectionValid,
  automationFormValid,
  availableAutomationModels,
  buildAutomationSubmission,
  eligibleAutomationAccounts,
  resolveAutomationModel,
  selectedAutomationAccounts,
} from "./automationModel";
export function AutomationsTable({ query, onEdit }: { query: string; onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} />;
  }
  const automations = runtime.automations.filter((task) => matchesQuery(query, task.name, task.accountSelector.kind === "all_eligible" ? "" : task.accountSelector.values, task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : ""));
  if (!automations.length) return <NoResults />;
  return (
    <div className="relay-table-wrap">
        <table className="relay-table">
          <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("connections.accounts")}</th><th>{t("common.model")}</th><th>{t("automations.lastResult")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
          <tbody>{automations.map((task) => {
            const history = runtime.wakeHistory.filter((item) => item.taskId === task.id);
            const last = history[history.length - 1];
            return (
              <tr key={task.id}>
                <td><input type="checkbox" checked={task.enabled} aria-label={t("common.enabled")} onChange={() => perform(`automation-${task.id}`, () => mode === "local" ? relayCommands.setAutomationEnabled(task.id, !task.enabled) : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled: !task.enabled }), "feedback.saved")} /></td>
                <td><strong>{task.name}</strong></td>
                <td>{task.accountSelector.kind === "all_eligible" ? t("automations.allEligible") : task.accountSelector.kind === "account_ids" ? task.accountSelector.values.map((id) => runtime.accounts.find((account) => account.id === id)?.label ?? id).join(", ") : task.accountSelector.values.join(", ")}</td>
                <td>{task.trigger.kind === "weekly" ? t("automations.weeklyReset") : task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</td>
                <td>{last ? t(`wake.${last.outcome}`, { defaultValue: last.outcome }) : t("common.never")}</td>
                <td className="row-actions-cell"><div className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem></ActionMenu></div></td>
              </tr>
            );
          })}</tbody>
        </table>
    </div>
  );
}

export function AutomationDialog({ task, onClose }: { task: WakeTask | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [name, setName] = useState(task?.name ?? t("automations.defaultName"));
  const [executionPolicy, setExecutionPolicy] = useState<WakeTask["executionPolicy"]>(mode === "local" ? task?.executionPolicy ?? "automatic" : "automatic");
  const [triggerKind, setTriggerKind] = useState<WakeTask["trigger"]["kind"]>(task?.trigger.kind ?? "quota_full");
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const accounts = runtime?.accounts ?? [];
  const poolAccounts = eligibleAutomationAccounts(accounts);
  const selectedAccounts = selectedAutomationAccounts(poolAccounts, accountIds);
  const weeklyReset = triggerKind === "weekly";
  const targetAccounts = selectorKind === "account_ids" ? selectedAccounts : selectorKind === "all_eligible" ? poolAccounts : [];
  const availableModels = runtime ? availableAutomationModels(runtime.gateway, targetAccounts, selectorKind) : [];
  const toggleAccount = (id: string) => setAccountIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  const accountSelectionValid = automationAccountSelectionValid(selectorKind, poolAccounts, accountIds, selectedAccounts);
  const selectedModel = resolveAutomationModel(availableModels, modelId);
  const valid = automationFormValid(name, accountSelectionValid, weeklyReset, selectedModel);
  const selectorOptions = [
    { value: "all_eligible", label: t("automations.allEligible") },
    { value: "account_ids", label: t("automations.selectedAccounts") },
    ...(selectorKind === "tags" ? [{ value: "tags", label: t("automations.matchingTags") }] : []),
  ];
  const triggerOptions = [
    { value: "quota_full", label: t("automations.primaryRecovery") },
    { value: "weekly", label: t("automations.weeklyReset") },
  ];
  const modelOptions = availableModels.length ? availableModels.map((model) => ({ value: model, label: model })) : [{ value: "", label: t("automations.noPoolModels") }];
  const save = async () => {
    if (!valid) return;
    const now = Date.now();
    const submission = buildAutomationSubmission({ task, name, executionPolicy, triggerKind, selectorKind, accountIds, selectedModel, nowMs: now });
    const ok = await perform(submission.operationId, () => mode === "local" ? (task ? relayCommands.updateAutomation(task.id, submission.base) : relayCommands.createAutomation(submission.base)) : relayCommands.remoteAction({ type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) }, submission.remoteInput), task ? "feedback.saved" : "feedback.automationAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form automation-form">
      <label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} autoFocus /></label>
      <div className="relay-field"><span>{t("automations.condition")}</span><OptionMenu className="field-option-menu" label={t("automations.condition")} value={triggerKind} onChange={(value) => setTriggerKind(value as WakeTask["trigger"]["kind"])} options={mode === "local" ? triggerOptions : triggerOptions.filter((option) => option.value === triggerKind)} /></div>
      <div className="automation-target-grid">
        <div className="relay-field"><span>{t("automations.accountSelection")}</span><OptionMenu className="field-option-menu" label={t("automations.accountSelection")} value={selectorKind} onChange={(value) => setSelectorKind(value as WakeTask["accountSelector"]["kind"])} options={selectorOptions} /></div>
        {!weeklyReset ? <div className="relay-field"><span>{t("common.model")}</span><OptionMenu className="field-option-menu" label={t("common.model")} value={selectedModel} onChange={setModelId} options={modelOptions} disabled={!availableModels.length} /></div> : null}
      </div>
      {selectorKind === "account_ids" ? <fieldset className="automation-account-picker"><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{poolAccounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} /><span>{account.label}</span></label>)}</div></fieldset> : null}
      {selectorKind === "tags" ? <><label className="relay-field"><span>{t("automations.tags")}</span><input value={task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : ""} readOnly /></label><p role="alert" className="automation-validation">{t("automations.legacyTags")}</p></> : null}
      {!accountSelectionValid ? <p role="alert" className="automation-validation">{t("automations.accountsRequired")}</p> : null}
      {!weeklyReset && accountSelectionValid && !selectedModel ? <p role="alert" className="automation-validation">{t("automations.modelUnavailable")}</p> : null}
      <div className="automation-rule">
        <span>{weeklyReset ? t("automations.weeklyCondition") : t("automations.execution")}</span>
        {!weeklyReset && mode === "local" ? <div className="segmented automation-execution" role="group" aria-label={t("automations.execution")}>
          <button type="button" className={executionPolicy === "automatic" ? "active" : ""} aria-pressed={executionPolicy === "automatic"} onClick={() => setExecutionPolicy("automatic")}>{t("automations.automatic")}</button>
          <button type="button" className={executionPolicy === "require_confirmation" ? "active" : ""} aria-pressed={executionPolicy === "require_confirmation"} onClick={() => setExecutionPolicy("require_confirmation")}>{t("automations.manual")}</button>
        </div> : <strong>{t("automations.automatic")}</strong>}
      </div>
      {mode !== "local" && task?.executionPolicy === "require_confirmation" ? <p role="status" className="automation-validation">{t("automations.remoteConfirmationMigration")}</p> : null}
    </div>
  </Dialog>;
}
