import { useEffect, useRef, useState } from "react";
import { BrainCircuit, ChevronDown, ChevronRight, CircleAlert, GripVertical } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { DefaultServiceTier, ModelSummary } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, ToggleSwitch } from "../../components/Ui";
import { currentPoolModelSummaries, groupModelSummaries } from "../../modelSummaries";
import { formatReasoningEffort } from "../../poolFormatting";
import {
  initialReasoningLevels,
  toggleReasoningLevel,
} from "./modelReasoningPolicy";
import {
  clearPendingModelEnabled,
  completeModelDisplayOrder,
  modelSignature,
  modelSpeedTiers,
  modelShowsContextControl,
  modelShowsReasoningControl,
  normalizeReasoningSelection,
  pendingModelEnabled,
  reconcilePendingModelEnabled,
  reorderById,
  reorderModelGroups,
  supportedReasoningLevels,
} from "./modelRulesModel";
import { useRelayState } from "../../state/RelayStateProvider";
import { PoolSpeedControl } from "./PoolSpeedControl";
import { PoolContextControl, type ModelContextPolicy } from "./PoolContextControl";
import { usePointerDragListeners } from "../../hooks/usePointerDragListeners";

type ModelDragState = {
  kind: "group" | "model";
  id: string;
  pointerId: number;
  clientX: number;
  clientY: number;
};

export function ModelRulesView() {
  const { t } = useTranslation();
  const { mode, runtime, perform } = useRelayState();
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  // Model Rules configures the complete pool inventory. Route health and
  // cooldowns are runtime state; they must not make a member's model vanish
  // from the policy editor while the relay can still recover or adapt it.
  const models = runtime ? currentPoolModelSummaries(runtime) : [];
  const [orderedModels, setOrderedModels] = useState<ModelSummary[]>(models);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const [pendingEnabled, setPendingEnabled] = useState<Record<string, boolean>>({});
  const [pendingSpeed, setPendingSpeed] = useState<Record<string, DefaultServiceTier>>({});
  const [pendingContext, setPendingContext] = useState<Record<string, ModelContextPolicy>>({});
  const [contextReset, setContextReset] = useState<Record<string, number>>({});
  const contextMutations = useRef(new Map<string, { next: ModelContextPolicy | null }>());
  const orderMutation = useRef(false);
  const latestModelsRef = useRef(models);
  latestModelsRef.current = models;
  const catalogSignature = modelSignature(models);
  useEffect(() => {
    setOrderedModels(models);
  }, [runtime?.configurationRevision, catalogSignature]);
  useEffect(() => {
    setPendingEnabled((previousPendingEnabled) => reconcilePendingModelEnabled(previousPendingEnabled, models));
  }, [catalogSignature]);
  useEffect(() => {
    setPendingContext((previous) => {
      let changed = false;
      const remaining = { ...previous };
      for (const model of latestModelsRef.current) {
        if (contextMutations.current.has(model.id)) continue;
        const saved = model.contextPolicy ?? { mode: "short" as const };
        if (remaining[model.id] !== undefined && JSON.stringify(remaining[model.id]) === JSON.stringify(saved)) {
          delete remaining[model.id];
          changed = true;
        }
      }
      return changed ? remaining : previous;
    });
  }, [catalogSignature]);
  useEffect(() => {
    setPendingSpeed((previousPendingSpeed) => {
      let changed = false;
      const remainingPendingTiers = { ...previousPendingSpeed };
      for (const model of latestModelsRef.current) {
        const savedTier = model.speedTier ?? "standard";
        if (remainingPendingTiers[model.id] !== undefined && remainingPendingTiers[model.id] === savedTier) {
          delete remainingPendingTiers[model.id];
          changed = true;
        }
      }
      return changed ? remainingPendingTiers : previousPendingSpeed;
    });
  }, [catalogSignature]);
  const modelGroups = groupModelSummaries(orderedModels, runtime?.accounts ?? []);
  const toggleModel = (model: ModelSummary) => {
    const enabled = !pendingModelEnabled(pendingEnabled, model);
    setPendingEnabled((previousPendingEnabled) => ({ ...previousPendingEnabled, [model.id]: enabled }));
    let saved = false;
    void perform(
      `model-toggle-${model.id}`,
      async () => {
        if (mode === "local") await relayCommands.setModelEnabled(model.id, enabled);
        else await relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled });
        saved = true;
      },
      "feedback.saved",
      { backgroundRefresh: true, uiLock: false },
    ).then((ok) => {
      if (saved || ok) return;
      setPendingEnabled((previousPendingEnabled) => clearPendingModelEnabled(previousPendingEnabled, model.id, enabled));
    });
  };
  const persistModelOrder = (orderedModelsToSave: ModelSummary[]) => perform(
    "model-order",
    () => mode === "local"
      ? relayCommands.setModelDisplayOrder(completeModelDisplayOrder(orderedModelsToSave, models))
      : relayCommands.remoteAction(
        { type: "set_model_order" },
        { modelIds: completeModelDisplayOrder(orderedModelsToSave, models) },
      ),
    "feedback.saved",
    { backgroundRefresh: true },
  );
  const saveModelOrder = async (orderedModelsToSave: ModelSummary[]) => {
    if (orderMutation.current) return;
    orderMutation.current = true;
    setOrderedModels(orderedModelsToSave);
    try {
      if (!await persistModelOrder(orderedModelsToSave)) setOrderedModels(latestModelsRef.current);
    } finally {
      orderMutation.current = false;
    }
  };
  const reorderModels = (sourceId: string, targetId: string) => {
    const sourceGroup = modelGroups.find((group) => group.models.some((model) => model.id === sourceId));
    if (!sourceGroup?.models.some((model) => model.id === targetId)) return;
    const reorderedModels = reorderById(orderedModels, sourceId, targetId);
    if (!reorderedModels) return;
    void saveModelOrder(reorderedModels);
  };
  const reorderGroups = (sourceId: string, targetId: string) => {
    const reorderedModels = reorderModelGroups(modelGroups, sourceId, targetId);
    if (!reorderedModels) return;
    void saveModelOrder(reorderedModels);
  };
  const setModelContext = (model: ModelSummary, nextPolicy: ModelContextPolicy) => {
    setPendingContext((previous) => ({ ...previous, [model.id]: nextPolicy }));
    const existing = contextMutations.current.get(model.id);
    if (existing) { existing.next = nextPolicy; return; }
    const mutation: { next: ModelContextPolicy | null } = { next: nextPolicy };
    contextMutations.current.set(model.id, mutation);
    void (async () => {
      let lastSaved = true;
      let submitted = nextPolicy;
      try {
        while (mutation.next) {
          submitted = mutation.next;
          mutation.next = null;
          const policy = submitted;
          lastSaved = await perform(`model-context-${model.id}`, () => mode === "local"
            ? relayCommands.setModelContext(model.id, policy)
            : relayCommands.remoteAction({ type: "set_model_context" }, { modelId: model.id, contextPolicy: policy }),
          "feedback.saved", { backgroundRefresh: true, uiLock: false });
        }
      } finally {
        contextMutations.current.delete(model.id);
        if (!lastSaved) setContextReset((previous) => ({ ...previous, [model.id]: (previous[model.id] ?? 0) + 1 }));
        const saved = latestModelsRef.current.find((item) => item.id === model.id)?.contextPolicy ?? { mode: "short" };
        if (!lastSaved || JSON.stringify(saved) === JSON.stringify(submitted)) {
          setPendingContext((previous) => {
            if (JSON.stringify(previous[model.id]) !== JSON.stringify(submitted)) return previous;
            const remaining = { ...previous };
            delete remaining[model.id];
            return remaining;
          });
        }
      }
    })();
  };
  const {
    dragModelId,
    dragGroupId,
    dropModelId,
    dropGroupId,
    startPointerDrag,
    startGroupDrag,
    startModelDrag,
    endGroupDrag,
    hoverGroup,
    dropGroup,
    endModelDrag,
    hoverModel,
    dropModel,
  } = useModelRuleDrag({ orderMutationRef: orderMutation, reorderModels, reorderGroups });
  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((previousCollapsedGroups) => ({
      ...previousCollapsedGroups,
      [groupId]: !previousCollapsedGroups[groupId],
    }));
  };
  if (!models.length) {
    return <div className="model-rules-empty"><EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} /></div>;
  }
  const catalogRefreshFailed = runtime?.warnings.some((warning) => warning.startsWith("model_catalog_refresh_failed"));
  const catalogReloadPending = runtime?.warnings.includes("model_catalog_refresh_deferred:codex_running");
  const contextStatus = catalogRefreshFailed ? t("pool.contextPolicies.refreshFailed")
    : catalogReloadPending ? t("pool.contextPolicies.codexRestart") : null;
  return <>
    <section className="model-rules relay-compact-content" aria-label={t("models.visible")}>
      {contextStatus ? <span className={`model-context-status${catalogRefreshFailed ? " error-text" : ""}`}
        role={catalogRefreshFailed ? "alert" : "status"} tabIndex={0}
        data-relay-tooltip={contextStatus} aria-label={contextStatus}
      ><CircleAlert aria-hidden /></span> : null}
      <div className="relay-table-wrap">
        <table className="relay-table model-rules-table">
          <colgroup><col data-column="model" /><col data-column="actions" /></colgroup>
          <thead><tr><th>{t("common.model")}</th><th>{t("common.actions")}</th></tr></thead>
          {modelGroups.map((group) => {
            const groupCollapsed = Boolean(collapsedGroups[group.id]);
            const groupLabel = t(`modelGroups.${group.id}`, { defaultValue: group.label });
            return <tbody key={group.id} id={`model-group-${group.id}`}>
              <tr
                className={`model-group-row${dragGroupId === group.id ? " model-dragging" : ""}`}
                data-group-id={group.id}
                data-drop-target={dropGroupId === group.id ? "true" : undefined}
                draggable
                onPointerDown={(event) => startPointerDrag(event, "group", group.id)}
                onDragStart={(event) => startGroupDrag(event, group.id)}
                onDragEnd={endGroupDrag}
                onDragOver={(event) => { event.preventDefault(); hoverGroup(group.id); }}
                onDrop={() => dropGroup(group.id)}
              >
                <th colSpan={2} scope="rowgroup">
                  <span className="model-group-content">
                    <button
                      className="model-group-toggle"
                      type="button"
                      aria-expanded={!groupCollapsed}
                      aria-controls={`model-group-${group.id}`}
                      aria-label={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })}
                      data-relay-tooltip={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })}
                      onClick={() => toggleGroup(group.id)}
                    >{groupCollapsed ? <ChevronRight aria-hidden /> : <ChevronDown aria-hidden />}</button>
                    <span className="model-group-drag-handle" data-relay-tooltip={t("models.dragGroup", { group: groupLabel })}><GripVertical aria-hidden /></span>
                    <strong>{groupLabel}</strong>
                    <small>{t("models.groupCount", { count: group.models.length })}</small>
                  </span>
                </th>
              </tr>
              {!groupCollapsed && group.models.map((model) => {
                const enabled = pendingModelEnabled(pendingEnabled, model);
                const displayName = model.catalogName || model.codexDisplayName || model.id;
                const toggleLabel = t(enabled ? "models.disable" : "models.enable", { model: model.id });
                const hasReasoningModes = modelShowsReasoningControl(model);
                const canEditReasoning = Boolean(model.reasoningConfigurable);
                const speedTiers = modelSpeedTiers(model);
                const requestedTier = pendingSpeed[model.id] ?? model.speedTier ?? "standard";
                const speedTier = speedTiers.includes(requestedTier) ? requestedTier : speedTiers[0] ?? "standard";
                const canEditContext = modelShowsContextControl(model);
                const canEditSpeed = model.speedSupported === true
                  && model.speedConfigurable === true
                  && speedTiers.length > 1;
                return <tr
                  key={model.id}
                  data-model-id={model.id}
                  data-enabled={enabled ? "true" : "false"}
                  data-drop-target={dropModelId === model.id ? "true" : undefined}
                  className={dragModelId === model.id ? "model-dragging" : undefined}
                  draggable
                  onPointerDown={(event) => startPointerDrag(event, "model", model.id)}
                  onDragStart={(event) => startModelDrag(event, model.id)}
                  onDragEnd={endModelDrag}
                  onDragOver={(event) => { event.preventDefault(); hoverModel(model.id); }}
                  onDrop={() => dropModel(model.id)}
                >
                  <td data-column="model">
                    <button
                      className="model-rule-drag-handle"
                      type="button"
                      aria-label={t("models.dragModel", { model: displayName })}
                      data-relay-tooltip={t("models.dragModel", { model: displayName })}
                      onPointerDown={(event) => startPointerDrag(event, "model", model.id)}
                    >
                      <GripVertical aria-hidden />
                    </button>
                    <div className="model-rule-identity">
                      <strong data-relay-tooltip={displayName}>{displayName}</strong>
                      {displayName !== model.id ? <code data-relay-tooltip={model.id}>{model.id}</code> : null}
                    </div>
                  </td>
                  <td data-column="actions"><div className="model-rule-actions">
                    {canEditSpeed ? <div className="model-rule-setting" data-setting="speed">
                      <PoolSpeedControl
                      className="model-speed-toggle"
                      iconsOnly
                      modelId={model.id}
                      value={speedTier}
                      tiers={speedTiers}
                      disabled={false}
                      saving={pendingSpeed[model.id] !== undefined && pendingSpeed[model.id] !== model.speedTier}
                      onChange={(nextTier) => {
                        setPendingSpeed((previousPendingSpeed) => ({ ...previousPendingSpeed, [model.id]: nextTier }));
                        void perform(`model-speed-${model.id}`, () => mode === "local"
                          ? relayCommands.setModelServiceTier(model.id, nextTier)
                          : relayCommands.remoteAction({ type: "set_model_service_tier" }, { modelId: model.id, serviceTier: nextTier }),
                        "feedback.saved",
                        { backgroundRefresh: true, uiLock: false },
                        ).then((ok) => {
                          if (ok) return;
                          setPendingSpeed((previousPendingSpeed) => {
                            if (previousPendingSpeed[model.id] !== nextTier) return previousPendingSpeed;
                            const remainingPendingTiers = { ...previousPendingSpeed };
                            delete remainingPendingTiers[model.id];
                            return remainingPendingTiers;
                          });
                        });
                      }} /></div> : null}
                    {canEditContext ? <div className="model-rule-setting" data-setting="context">
                      <PoolContextControl
                      className="model-context-toggle"
                      value={pendingContext[model.id] ?? model.contextPolicy ?? { mode: "short" }}
                      maximumTokens={(model.contextWindows ? model.contextWindows.maximumTokens : model.catalogContextLimit) ?? undefined}
                      defaultTokens={model.contextWindows?.defaultTokens ?? undefined}
                      resetRevision={contextReset[model.id] ?? 0}
                      disabled={false}
                      saving={pendingContext[model.id] !== undefined && JSON.stringify(pendingContext[model.id]) !== JSON.stringify(model.contextPolicy ?? { mode: "short" })}
                      onChange={(nextPolicy) => setModelContext(model, nextPolicy)}
                    /></div> : null}
                    {hasReasoningModes ? <IconButton
                      data-model-reasoning-edit={model.id}
                      label={t(canEditReasoning ? "models.editReasoning" : "models.viewReasoning", { model: model.id })}
                      icon={<BrainCircuit aria-hidden />}
                      onClick={() => setReasoningModel(model)}
                    /> : null}
                    <ToggleSwitch
                      data-model-toggle={model.id}
                      label={toggleLabel}
                      className="model-toggle"
                      checked={enabled}
                      onChange={() => void toggleModel(model)}
                    />
                  </div></td>
                </tr>;
              })}
            </tbody>;
          })}
        </table>
      </div>
    </section>
    {reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}
  </>;
}

function useModelRuleDrag({
  orderMutationRef,
  reorderModels,
  reorderGroups,
}: {
  orderMutationRef: { current: boolean };
  reorderModels: (sourceId: string, targetId: string) => void;
  reorderGroups: (sourceId: string, targetId: string) => void;
}) {
  const [dragModelId, setDragModelId] = useState<string | null>(null);
  const [dragGroupId, setDragGroupId] = useState<string | null>(null);
  const [dropModelId, setDropModelId] = useState<string | null>(null);
  const [dropGroupId, setDropGroupId] = useState<string | null>(null);
  const modelDragRef = useRef<ModelDragState | null>(null);
  const clearModelDrag = () => {
    modelDragRef.current = null;
    setDragModelId(null);
    setDragGroupId(null);
    setDropModelId(null);
    setDropGroupId(null);
  };
  const updateModelDragAt = (clientX: number, clientY: number) => {
    const drag = modelDragRef.current;
    if (!drag) return;
    const targetElement = document.elementFromPoint(clientX, clientY);
    if (drag.kind === "group") {
      const targetId = targetElement?.closest<HTMLElement>("[data-group-id]")?.dataset["groupId"] ?? null;
      setDropGroupId(targetId && targetId !== drag.id ? targetId : null);
      setDropModelId(null);
      return;
    }
    const targetId = targetElement?.closest<HTMLElement>("[data-model-id]")?.dataset["modelId"] ?? null;
    setDropModelId(targetId && targetId !== drag.id ? targetId : null);
    setDropGroupId(null);
  };
  const finishModelDragAt = (clientX: number, clientY: number) => {
    const drag = modelDragRef.current;
    if (!drag) return;
    const targetElement = document.elementFromPoint(clientX, clientY);
    if (drag.kind === "group") {
      const targetId = targetElement?.closest<HTMLElement>("[data-group-id]")?.dataset["groupId"];
      if (targetId && targetId !== drag.id) reorderGroups(drag.id, targetId);
    } else {
      const targetId = targetElement?.closest<HTMLElement>("[data-model-id]")?.dataset["modelId"];
      if (targetId && targetId !== drag.id) reorderModels(drag.id, targetId);
    }
    clearModelDrag();
  };
  usePointerDragListeners({
    dragRef: modelDragRef,
    activeKey: dragModelId ? `model:${dragModelId}` : dragGroupId ? `group:${dragGroupId}` : null,
    onMove: (_drag, clientX, clientY) => updateModelDragAt(clientX, clientY),
    onDrop: (_drag, clientX, clientY) => finishModelDragAt(clientX, clientY),
    onCancel: clearModelDrag,
  });
  const startPointerDrag = (event: React.PointerEvent<HTMLElement>, kind: ModelDragState["kind"], draggedItemId: string) => {
    if (orderMutationRef.current) return;
    const targetElement = event.target as HTMLElement;
    if (event.button !== 0 || (targetElement.closest(".pool-speed-control, .pool-context-control, button, input, textarea, a") && !targetElement.closest(".model-rule-drag-handle, .model-group-drag-handle"))) return;
    event.preventDefault();
    modelDragRef.current = { kind, id: draggedItemId, pointerId: event.pointerId, clientX: event.clientX, clientY: event.clientY };
    setDragModelId(kind === "model" ? draggedItemId : null);
    setDragGroupId(kind === "group" ? draggedItemId : null);
    setDropModelId(null);
    setDropGroupId(null);
  };
  const startGroupDrag = (event: React.DragEvent<HTMLTableRowElement>, groupId: string) => {
    if (orderMutationRef.current) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", `group:${groupId}`);
    setDragGroupId(groupId);
    setDropGroupId(null);
  };
  const startModelDrag = (event: React.DragEvent<HTMLTableRowElement>, modelId: string) => {
    // Interactive controls inside a draggable row must keep their normal
    // click/focus behavior; the row itself is the drag surface.
    if (orderMutationRef.current || (event.target as HTMLElement).closest(".pool-speed-control, .pool-context-control, button, input, textarea, a")) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", `model:${modelId}`);
    setDragModelId(modelId);
    setDropModelId(null);
  };
  return {
    dragModelId,
    dragGroupId,
    dropModelId,
    dropGroupId,
    startPointerDrag,
    startGroupDrag,
    startModelDrag,
    endGroupDrag: () => { setDragGroupId(null); setDropGroupId(null); },
    hoverGroup: (groupId: string) => setDropGroupId(dragGroupId && dragGroupId !== groupId ? groupId : null),
    dropGroup: (groupId: string) => {
      if (dragGroupId) reorderGroups(dragGroupId, groupId);
      setDragGroupId(null);
      setDropGroupId(null);
    },
    endModelDrag: () => { setDragModelId(null); setDropModelId(null); },
    hoverModel: (modelId: string) => setDropModelId(dragModelId && dragModelId !== modelId ? modelId : null),
    dropModel: (modelId: string) => {
      if (dragModelId) reorderModels(dragModelId, modelId);
      setDragModelId(null);
      setDropModelId(null);
    },
  };
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform } = useRelayState();
  // The backend owns the provider contract and its order. Never synthesize
  // or reorder levels in the editor, and discard stale custom values from old
  // local policies before they can be sent back to the runtime.
  const supportedLevels = supportedReasoningLevels(model);
  const editable = Boolean(model.reasoningConfigurable);
  const normalizeToSupported = (levels: string[]) => {
    return normalizeReasoningSelection(supportedLevels, levels);
  };
  const [allowedLevels, setAllowedLevels] = useState(() => normalizeToSupported(
    initialReasoningLevels(model.reasoningAllowedLevels, model.reasoningLevels ?? []),
  ));
  const allowedLevelsRef = useRef(allowedLevels);
  const policyRevision = useRef(0);
  const mutationLock = useRef(false);
  const queuedLevels = useRef<string[] | null>(null);
  const operation = `model-reasoning-${model.id}`;
  const label = (level: string) => t(`usage.reasoningEfforts.${level}`, { defaultValue: formatReasoningEffort(level) });
  const updateAllowedLevels = (updatedLevels: string[]) => {
    allowedLevelsRef.current = updatedLevels;
    setAllowedLevels(updatedLevels);
  };
  const runSerialized = async (operationId: string, work: () => Promise<unknown>, successKey?: string) => {
    if (mutationLock.current) return false;
    mutationLock.current = true;
    try {
      return await perform(operationId, work, successKey, { backgroundRefresh: true, uiLock: false });
    } finally {
      mutationLock.current = false;
    }
  };
  const saveLevels = async (levelsToSave: string[]) => {
    const normalized = normalizeToSupported(levelsToSave);
    const previousAllowedLevels = allowedLevelsRef.current;
    if (normalized.join("\0") === previousAllowedLevels.join("\0")) return;
    policyRevision.current += 1;
    updateAllowedLevels(normalized);
    queuedLevels.current = normalized;
    if (mutationLock.current) return;
    let confirmedAllowedLevels = previousAllowedLevels;
    while (queuedLevels.current) {
      const queuedLevelsSnapshot = queuedLevels.current;
      const queuedPolicyRevision = policyRevision.current;
      queuedLevels.current = null;
      const ok = await runSerialized(operation, () => mode === "local"
        ? relayCommands.setModelReasoning(model.id, queuedLevelsSnapshot)
        : relayCommands.remoteAction({ type: "set_model_reasoning" }, { modelId: model.id, allowedLevels: queuedLevelsSnapshot }), "feedback.saved");
      if (ok) confirmedAllowedLevels = queuedLevelsSnapshot;
      else if (queuedPolicyRevision === policyRevision.current && queuedLevels.current === null) {
        updateAllowedLevels(confirmedAllowedLevels);
        return;
      }
    }
  };
  const toggleAllowedLevel = (level: string) => {
    const updatedLevels = normalizeToSupported(toggleReasoningLevel(allowedLevelsRef.current, level));
    if (updatedLevels === allowedLevelsRef.current) return;
    void saveLevels(updatedLevels);
  };
  return <Dialog className="model-reasoning-dialog" title={t("models.reasoningTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <div className="model-reasoning-form">
      <code className="model-reasoning-model" data-relay-tooltip={model.id}>{model.id}</code>
      <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningTitle")}>
        {supportedLevels.map((level) => <button
          key={level}
          type="button"
          role="checkbox"
          aria-checked={allowedLevels.includes(level)}
          className={allowedLevels.includes(level) ? "selected" : undefined}
          disabled={!editable}
          onClick={() => toggleAllowedLevel(level)}
        ><span>{label(level)}</span></button>)}
      </div>
    </div>
  </Dialog>;
}
