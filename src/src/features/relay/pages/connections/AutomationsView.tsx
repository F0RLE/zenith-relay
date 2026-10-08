import { useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { WakeTask } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, ToggleSwitch, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";
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

function AutomationEnabledControl({ task }: { task: WakeTask }) {
  const { t } = useTranslation();
  const { mode, perform } = useRelayState();
  const pending = usePendingFlag(task.enabled);
  return <ToggleSwitch
    checked={pending.checked}
    label={t("common.enabled")}
    aria-busy={pending.checked !== task.enabled}
    onChange={(enabled) => pending.select(enabled, () => perform(
      `automation-${task.id}`,
      () => mode === "local"
        ? relayCommands.setAutomationEnabled(task.id, enabled)
        : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled, executionPolicy: "automatic" }),
      "feedback.saved",
      { backgroundRefresh: true, uiLock: false },
    ))}
  />;
}

export function AutomationsList({ onEdit }: { onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} />;
  }
  return (
    <div className="automation-list connection-list-wrap" role="list">
          {runtime.automations.map((task) => {
            const triggerType = automationType(task.trigger.kind);
            const automationName = automationDisplayName(task, t);
            const triggerTypeName = t(triggerType.nameKey);
            const history = runtime.wakeHistory.filter((wakeEvent) => wakeEvent.taskId === task.id);
            const lastWakeEvent = history[history.length - 1];
            return (
              <article className="automation-card" role="listitem" key={task.id}>
                <header>
                  <AutomationEnabledControl task={task} />
                  <div className="connection-identity">
              <strong>{automationName}</strong>
                    {automationName !== triggerTypeName ? <small>{triggerTypeName}</small> : null}
                  </div>
                  <div className="row-actions">
                    <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} />
                    <ActionMenu>
                      <ActionMenuItem
                        danger
                        icon={<Trash2 aria-hidden />}
                        onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(
                          `delete-${task.id}`,
                          () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }),
                          "feedback.deleted",
                          { backgroundRefresh: true },
                        ))}
                      >
                        {t("common.delete")}
                      </ActionMenuItem>
                    </ActionMenu>
                  </div>
                </header>
                <dl className="automation-details">
                  <div>
                    <dt>{t("automations.condition")}</dt>
                    <dd>
                      {t(triggerType.conditionKey)}
                      {triggerType.requiresModel ? <small>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</small> : null}
                    </dd>
                  </div>
                  <div>
                    <dt>{t("connections.accounts")}</dt>
                    <dd>
                      {task.accountSelector.kind === "all_eligible"
                        ? t("automations.allEligible")
                        : task.accountSelector.kind === "account_ids"
                          ? task.accountSelector.values.map((accountId) => runtime.accounts.find((account) => account.id === accountId)?.label ?? t("accounts.importUnknownAccount")).join(", ")
                          : task.accountSelector.values.join(", ")}
                    </dd>
                  </div>
                  <div>
                    <dt>{t("automations.lastResult")}</dt>
                    <dd>{lastWakeEvent ? t(`wake.${lastWakeEvent.outcome}`, { defaultValue: lastWakeEvent.outcome }) : t("common.never")}</dd>
                  </div>
                </dl>
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
  const automationName = customName?.trim() || defaultAutomationName(triggerKind, t);
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const accounts = runtime?.accounts ?? [];
  const poolAccounts = eligibleAutomationAccounts(accounts);
  const selectedAccounts = selectedAutomationAccounts(poolAccounts, accountIds);
  const triggerType = automationType(triggerKind);
  const targetAccounts = selectorKind === "account_ids" ? selectedAccounts : selectorKind === "all_eligible" ? poolAccounts : [];
  const availableModels = runtime ? availableAutomationModels(runtime.gateway, targetAccounts, selectorKind) : [];
  const toggleAccount = (accountId: string) => setAccountIds((previousAccountIds) => previousAccountIds.includes(accountId)
    ? previousAccountIds.filter((selectedId) => selectedId !== accountId)
    : [...previousAccountIds, accountId]);
  const accountSelectionValid = automationAccountSelectionValid(selectorKind, poolAccounts, accountIds, selectedAccounts);
  const selectedModel = resolveAutomationModel(availableModels, modelId);
  const valid = automationFormValid(automationName, accountSelectionValid, triggerType.requiresModel, selectedModel);
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
    const submission = buildAutomationSubmission({ task, automationName, triggerKind, selectorKind, accountIds, selectedModel, nowMs: now });
    const ok = await perform(
      submission.operationId,
      () => mode === "local"
        ? task
          ? relayCommands.updateAutomation(task.id, submission.base)
          : relayCommands.createAutomation(submission.base)
        : relayCommands.remoteAction(
          { type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) },
          submission.remoteInput,
        ),
      task ? "feedback.saved" : "feedback.automationAdded",
      { backgroundRefresh: true },
    );
    if (ok) onClose();
  };
  return (
    <Dialog
      wide
      className="connection-dialog automation-dialog"
      title={task ? t("automations.edit") : t("automations.add")}
      onClose={onClose}
      footer={(
        <>
          <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
          <Button
            variant="primary"
            busy={busy === (task ? `automation-update-${task.id}` : "automation-create")}
            disabled={!valid}
            onClick={save}
          >
            {t("common.save")}
          </Button>
        </>
      )}
    >
    <div className="relay-form automation-form">
      <div className="relay-field">
        <span>{t("automations.type")}</span>
        <OptionMenu
          className="field-option-menu"
          label={t("automations.type")}
          value={triggerKind}
          onChange={(triggerKindValue) => setTriggerKind(triggerKindValue as WakeTask["trigger"]["kind"])}
          options={typeOptions}
        />
                <small className="automation-trigger-note">{t(triggerType.conditionKey)}</small>
      </div>
      <div className="automation-target-grid">
        <div className="relay-field">
          <span>{t("automations.accountSelection")}</span>
          <OptionMenu
            className="field-option-menu"
            label={t("automations.accountSelection")}
            value={selectorKind}
            onChange={(selectorKindValue) => setSelectorKind(selectorKindValue as WakeTask["accountSelector"]["kind"])}
            options={selectorOptions}
          />
        </div>
        {triggerType.requiresModel ? (
          <div className="relay-field">
            <span>{t("common.model")}</span>
            <OptionMenu
              className="field-option-menu"
              label={t("common.model")}
              value={selectedModel}
              onChange={setModelId}
              options={modelOptions}
              disabled={!availableModels.length}
            />
          </div>
        ) : null}
      </div>
      {selectorKind === "account_ids" ? (
        <fieldset className="automation-account-picker">
          <legend>{t("automations.selectedAccounts")}</legend>
          <div className="scope-grid">
            {poolAccounts.map((account) => (
              <label key={account.id}>
                <input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} />
                <span>{account.label}</span>
              </label>
            ))}
          </div>
        </fieldset>
      ) : null}
      {selectorKind === "tags" ? (
        <>
          <label className="relay-field">
            <span>{t("automations.tags")}</span>
            <input value={task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : ""} readOnly />
          </label>
          <p role="alert" className="automation-validation">{t("automations.legacyTags")}</p>
        </>
      ) : null}
      <label className="relay-field">
        <span>{t("common.name")}</span>
        <input
          value={customName ?? ""}
          placeholder={t("automations.optionalName")}
          onChange={(event) => setCustomName(event.target.value)}
        />
      </label>
      {!accountSelectionValid ? <p role="alert" className="automation-validation">{t("automations.accountsRequired")}</p> : null}
      {triggerType.requiresModel && accountSelectionValid && !selectedModel ? <p role="alert" className="automation-validation">{t("automations.modelUnavailable")}</p> : null}
    </div>
    </Dialog>
  );
}
