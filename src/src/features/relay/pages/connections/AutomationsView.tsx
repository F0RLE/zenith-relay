import { useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { automationDefaultNames } from "../../../../i18n/automationNames";
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
  weeklyResetThresholdBasisPoints,
} from "./automationModel";

function AutomationEnabledControl({ task }: { task: WakeTask }) {
  const { t } = useTranslation();
  const { perform } = useRelayState();
  const pending = usePendingFlag(task.enabled);
  return <ToggleSwitch
    checked={pending.checked}
    label={t("common.enabled")}
    aria-busy={pending.checked !== task.enabled}
    onChange={(enabled) => pending.select(enabled, () => perform(
      `automation-${task.id}`,
      () => relayCommands.setAutomationEnabled(task.id, enabled),
      "feedback.saved",
      { backgroundRefresh: true, uiLock: false },
    ))}
  />;
}

export function AutomationsList({ onEdit }: { onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { runtime, perform } = useRelayState();
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
            const poolAccounts = eligibleAutomationAccounts(runtime.accounts);
            const targetAccounts = task.accountSelector.kind === "account_ids"
              ? selectedAutomationAccounts(poolAccounts, task.accountSelector.values)
              : poolAccounts;
            const availableModels = availableAutomationModels(runtime.gateway, targetAccounts, task.accountSelector.kind);
            const modelUnavailable = triggerType.requiresModel
              && task.modelPolicy.kind === "explicit"
              && !resolveAutomationModel(availableModels, task.modelPolicy.value);
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
                          () => relayCommands.deleteAutomation(task.id),
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
                      {task.trigger.kind === "weekly" && task.weeklyResetRemainingBasisPoints != null
                        ? t("automations.weeklyThresholdCondition", { percent: task.weeklyResetRemainingBasisPoints / 100 })
                        : t(triggerType.conditionKey)}
                      {triggerType.requiresModel ? <small>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</small> : null}
                      {modelUnavailable ? <small className="automation-validation">{t("automations.selectedModelUnavailable")}</small> : null}
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
  const [weeklyThresholdEnabled, setWeeklyThresholdEnabled] = useState(task?.weeklyResetRemainingBasisPoints != null);
  const [weeklyThresholdPercent, setWeeklyThresholdPercent] = useState(String((task?.weeklyResetRemainingBasisPoints ?? 0) / 100));
  const weeklyThreshold = weeklyResetThresholdBasisPoints(weeklyThresholdPercent);
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
  const modelUnavailable = Boolean(modelId.trim() && !selectedModel);
  const valid = automationFormValid(automationName, accountSelectionValid, triggerType.requiresModel, selectedModel)
    && (triggerKind !== "weekly" || !weeklyThresholdEnabled || weeklyThreshold != null);
  const selectorOptions = [
    { value: "all_eligible", label: t("automations.allEligible") },
    { value: "account_ids", label: t("automations.selectedAccounts") },
    ...(selectorKind === "tags" ? [{ value: "tags", label: t("automations.matchingTags") }] : []),
  ];
  const typeOptions = automationTypes
    .filter((option) => mode === "local" || option.triggerKind === triggerKind)
    .map((option) => ({ value: option.triggerKind, label: t(option.nameKey) }));
  const modelOptions = [
    ...(modelUnavailable ? [{ value: modelId, label: t("automations.unavailableModel", { model: modelId }) }] : []),
    ...availableModels.map((model) => ({ value: model, label: model })),
  ];
  if (!modelOptions.length) modelOptions.push({ value: "", label: t("automations.noPoolModels") });
  const save = async () => {
    if (!valid) return;
    const storedName = customName?.trim() || (triggerKind === "weekly" ? automationDefaultNames.en.weeklyDefaultName : automationDefaultNames.en.defaultName);
    const submission = buildAutomationSubmission({
      task, automationName: storedName, triggerKind, selectorKind, accountIds, selectedModel,
      weeklyResetRemainingBasisPoints: weeklyThresholdEnabled ? weeklyThreshold : null,
    });
    const ok = await perform(
      submission.operationId,
      () => task
        ? relayCommands.updateAutomation(task.id, submission.base)
        : relayCommands.createAutomation(submission.base),
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
        <small className="automation-trigger-note">{triggerKind === "weekly" && weeklyThresholdEnabled
          ? weeklyThreshold != null
            ? t("automations.weeklyThresholdCondition", { percent: weeklyThreshold / 100 })
            : t("automations.weeklyThreshold")
          : t(triggerType.conditionKey)}</small>
      </div>
      <div className="automation-target-grid">
        {triggerKind === "weekly" ? <>
          <div className="relay-field">
            <span>{t("automations.weeklyTiming")}</span>
            <OptionMenu className="field-option-menu" label={t("automations.weeklyTiming")}
              value={weeklyThresholdEnabled ? "threshold" : "reserve"}
              onChange={(value) => setWeeklyThresholdEnabled(value === "threshold")}
              options={[
                { value: "reserve", label: t("automations.weeklyReserve") },
                { value: "threshold", label: t("automations.weeklyThreshold") },
              ]} />
          </div>
          {weeklyThresholdEnabled ? <label className="relay-field">
            <span>{t("automations.weeklyRemainingPercent")}</span>
            <input type="number" min={0} max={100} step={1} inputMode="numeric"
              value={weeklyThresholdPercent}
              onChange={(event) => setWeeklyThresholdPercent(event.target.value)}
              aria-invalid={weeklyThreshold == null}
              aria-describedby="weekly-reset-threshold-help" />
            <small id="weekly-reset-threshold-help">{t("automations.weeklyThresholdHelp")}</small>
            {weeklyThreshold == null ? <small role="alert">{t("automations.weeklyThresholdInvalid")}</small> : null}
          </label> : null}
        </> : null}
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
              value={modelUnavailable ? modelId : selectedModel}
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
      {triggerType.requiresModel && accountSelectionValid && !selectedModel ? <p role="alert" className="automation-validation">{t(modelUnavailable ? "automations.selectedModelUnavailable" : "automations.modelUnavailable")}</p> : null}
    </div>
    </Dialog>
  );
}
