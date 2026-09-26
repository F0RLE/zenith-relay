import { useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { WakeTask } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, ToggleSwitch, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import {
  automationAccountSelectionValid,
  automationDisplayName,
  automationFormValid,
  automationType,
  automationTypes,
  availableAutomationModels,
  buildAutomationSubmission,
  customAutomationName,
  defaultAutomationName,
  eligibleAutomationAccounts,
  resolveAutomationModel,
  selectedAutomationAccounts,
} from "./automationModel";
export function AutomationsList({ onEdit }: { onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} />;
  }
  return (
    <div className="automation-list connection-list-wrap" role="list">
          {runtime.automations.map((task) => {
            const type = automationType(task.trigger.kind);
            const name = automationDisplayName(task, t);
            const typeName = t(type.nameKey);
            const history = runtime.wakeHistory.filter((item) => item.taskId === task.id);
            const last = history[history.length - 1];
            return (
              <article className="automation-card" role="listitem" key={task.id}>
                <header><ToggleSwitch checked={task.enabled} label={t("common.enabled")} disabled={Boolean(busy)} aria-busy={busy === `automation-${task.id}`} onChange={() => void perform(`automation-${task.id}`, () => mode === "local" ? relayCommands.setAutomationEnabled(task.id, !task.enabled) : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled: !task.enabled, executionPolicy: "automatic" }), "feedback.saved")} /><div className="connection-identity"><strong>{name}</strong>{name !== typeName ? <small>{typeName}</small> : null}</div>
                <div className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem></ActionMenu></div></header>
                <dl className="automation-details"><div><dt>{t("automations.condition")}</dt><dd>{t(type.conditionKey)}{type.requiresModel ? <small>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</small> : null}</dd></div>
                <div><dt>{t("connections.accounts")}</dt><dd>{task.accountSelector.kind === "all_eligible" ? t("automations.allEligible") : task.accountSelector.kind === "account_ids" ? task.accountSelector.values.map((id) => runtime.accounts.find((account) => account.id === id)?.label ?? t("accounts.importUnknownAccount")).join(", ") : task.accountSelector.values.join(", ")}</dd></div>
                <div><dt>{t("automations.lastResult")}</dt><dd>{last ? t(`wake.${last.outcome}`, { defaultValue: last.outcome }) : t("common.never")}</dd></div></dl>
              </article>
            );
          })}
    </div>
  );
}

export function AutomationDialog({ task, onClose }: { task: WakeTask | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [customName, setCustomName] = useState(() => customAutomationName(task?.name));
  const [triggerKind, setTriggerKind] = useState<WakeTask["trigger"]["kind"]>(task?.trigger.kind ?? "quota_full");
  const name = customName?.trim() || defaultAutomationName(triggerKind, t);
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const accounts = runtime?.accounts ?? [];
  const poolAccounts = eligibleAutomationAccounts(accounts);
  const selectedAccounts = selectedAutomationAccounts(poolAccounts, accountIds);
  const type = automationType(triggerKind);
  const targetAccounts = selectorKind === "account_ids" ? selectedAccounts : selectorKind === "all_eligible" ? poolAccounts : [];
  const availableModels = runtime ? availableAutomationModels(runtime.gateway, targetAccounts, selectorKind) : [];
  const toggleAccount = (id: string) => setAccountIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  const accountSelectionValid = automationAccountSelectionValid(selectorKind, poolAccounts, accountIds, selectedAccounts);
  const selectedModel = resolveAutomationModel(availableModels, modelId);
  const valid = automationFormValid(name, accountSelectionValid, type.requiresModel, selectedModel);
  const selectorOptions = [
    { value: "all_eligible", label: t("automations.allEligible") },
    { value: "account_ids", label: t("automations.selectedAccounts") },
    ...(selectorKind === "tags" ? [{ value: "tags", label: t("automations.matchingTags") }] : []),
  ];
  const typeOptions = automationTypes
    .filter((option) => mode === "local" || option.triggerKind === triggerKind)
    .map((option) => ({ value: option.triggerKind, label: t(option.nameKey) }));
  const modelOptions = availableModels.length ? availableModels.map((model) => ({ value: model, label: model })) : [{ value: "", label: t("automations.noPoolModels") }];
  const save = async () => {
    if (!valid) return;
    const now = Date.now();
    const submission = buildAutomationSubmission({ task, name, triggerKind, selectorKind, accountIds, selectedModel, nowMs: now });
    const ok = await perform(submission.operationId, () => mode === "local" ? (task ? relayCommands.updateAutomation(task.id, submission.base) : relayCommands.createAutomation(submission.base)) : relayCommands.remoteAction({ type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) }, submission.remoteInput), task ? "feedback.saved" : "feedback.automationAdded");
    if (ok) onClose();
  };
  return <Dialog wide className="connection-dialog automation-dialog" title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form automation-form">
      <div className="relay-field"><span>{t("automations.type")}</span><OptionMenu className="field-option-menu" label={t("automations.type")} value={triggerKind} onChange={(value) => setTriggerKind(value as WakeTask["trigger"]["kind"])} options={typeOptions} /><small className="automation-trigger-note">{t(type.conditionKey)}</small></div>
      <div className="automation-target-grid">
        <div className="relay-field"><span>{t("automations.accountSelection")}</span><OptionMenu className="field-option-menu" label={t("automations.accountSelection")} value={selectorKind} onChange={(value) => setSelectorKind(value as WakeTask["accountSelector"]["kind"])} options={selectorOptions} /></div>
        {type.requiresModel ? <div className="relay-field"><span>{t("common.model")}</span><OptionMenu className="field-option-menu" label={t("common.model")} value={selectedModel} onChange={setModelId} options={modelOptions} disabled={!availableModels.length} /></div> : null}
      </div>
      {selectorKind === "account_ids" ? <fieldset className="automation-account-picker"><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{poolAccounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} /><span>{account.label}</span></label>)}</div></fieldset> : null}
      {selectorKind === "tags" ? <><label className="relay-field"><span>{t("automations.tags")}</span><input value={task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : ""} readOnly /></label><p role="alert" className="automation-validation">{t("automations.legacyTags")}</p></> : null}
      <label className="relay-field"><span>{t("common.name")}</span><input value={customName ?? ""} placeholder={t("automations.optionalName")} onChange={(event) => setCustomName(event.target.value)} /></label>
      {!accountSelectionValid ? <p role="alert" className="automation-validation">{t("automations.accountsRequired")}</p> : null}
      {type.requiresModel && accountSelectionValid && !selectedModel ? <p role="alert" className="automation-validation">{t("automations.modelUnavailable")}</p> : null}
    </div>
  </Dialog>;
}
