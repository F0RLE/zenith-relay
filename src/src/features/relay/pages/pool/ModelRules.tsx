import { useEffect, useRef, useState } from "react";
import { BrainCircuit, ChevronDown, ChevronRight, GripVertical, Loader2, Power, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { DefaultServiceTier, ModelSummary } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, OptionMenu } from "../../components/Ui";
import { currentPoolModelSummaries, groupModelSummaries, operationalModelSummaries } from "../../poolHelpers";
import { formatReasoningEffort } from "../../poolFormatting";
import {
  initialReasoningLevels,
  toggleReasoningLevel,
} from "./modelReasoningPolicy";
import {
  completeModelDisplayOrder,
  modelSignature,
  normalizeReasoningSelection,
  reorderById,
  reorderModelGroups,
  supportedReasoningLevels,
} from "./modelRulesModel";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePointerDragListeners } from "../../hooks/usePointerDragListeners";

type ModelDragState = {
  kind: "group" | "model";
  id: string;
  pointerId: number;
  clientX: number;
  clientY: number;
};

function modelSpeedTiers(model: ModelSummary, current: DefaultServiceTier): DefaultServiceTier[] {
  const declared: DefaultServiceTier[] = model.speedTiers?.length
    ? model.speedTiers
    : model.speedSupported
      ? ["standard", "fast"] satisfies DefaultServiceTier[]
      : ["standard"] satisfies DefaultServiceTier[];
  return Array.from(new Set<DefaultServiceTier>([...declared, current]));
}

export function ModelRulesView() {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  const models = runtime ? operationalModelSummaries(runtime) : [];
  const poolModels = runtime ? currentPoolModelSummaries(runtime) : [];
  const [orderedModels, setOrderedModels] = useState<ModelSummary[]>(models);
  const [dragModelId, setDragModelId] = useState<string | null>(null);
  const [dragGroupId, setDragGroupId] = useState<string | null>(null);
  const [dropModelId, setDropModelId] = useState<string | null>(null);
  const [dropGroupId, setDropGroupId] = useState<string | null>(null);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const modelDragRef = useRef<ModelDragState | null>(null);
  const catalogSignature = modelSignature(models);
  useEffect(() => {
    setOrderedModels(models);
  }, [runtime?.configurationRevision, catalogSignature]);
  const modelGroups = groupModelSummaries(orderedModels, runtime?.accounts ?? []);
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  const saveModelOrder = (next: ModelSummary[]) => perform(
    "model-order",
    () => mode === "local"
      ? relayCommands.setModelDisplayOrder(completeModelDisplayOrder(next, poolModels))
      : relayCommands.remoteAction(
        { type: "set_model_order" },
        { modelIds: completeModelDisplayOrder(next, poolModels) },
      ),
    "feedback.saved",
  );
  const reorderModels = (sourceId: string, targetId: string) => {
    const next = reorderById(orderedModels, sourceId, targetId);
    if (!next) return;
    setOrderedModels(next);
    void saveModelOrder(next);
  };
  const reorderGroups = (sourceId: string, targetId: string) => {
    const next = reorderModelGroups(modelGroups, sourceId, targetId);
    if (!next) return;
    setOrderedModels(next);
    void saveModelOrder(next);
  };
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
    const target = document.elementFromPoint(clientX, clientY);
    if (drag.kind === "group") {
      const row = target?.closest<HTMLElement>("[data-group-id]");
      const targetId = row?.dataset["groupId"] ?? null;
      setDropGroupId(targetId && targetId !== drag.id ? targetId : null);
      setDropModelId(null);
      return;
    }
    const row = target?.closest<HTMLElement>("[data-model-id]");
    const targetId = row?.dataset["modelId"] ?? null;
    setDropModelId(targetId && targetId !== drag.id ? targetId : null);
    setDropGroupId(null);
  };
  const finishModelDragAt = (clientX: number, clientY: number) => {
    const drag = modelDragRef.current;
    if (!drag) return;
    const target = document.elementFromPoint(clientX, clientY);
    if (drag.kind === "group") {
      const targetId = target?.closest<HTMLElement>("[data-group-id]")?.dataset["groupId"];
      if (targetId && targetId !== drag.id) reorderGroups(drag.id, targetId);
    } else {
      const targetId = target?.closest<HTMLElement>("[data-model-id]")?.dataset["modelId"];
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
  const startPointerDrag = (event: React.PointerEvent<HTMLElement>, kind: ModelDragState["kind"], id: string) => {
    const target = event.target as HTMLElement;
    if (event.button !== 0 || (target.closest("button, input, textarea, a") && !target.closest(".model-rule-drag-handle, .model-group-drag-handle"))) return;
    event.preventDefault();
    modelDragRef.current = { kind, id, pointerId: event.pointerId, clientX: event.clientX, clientY: event.clientY };
    setDragModelId(kind === "model" ? id : null);
    setDragGroupId(kind === "group" ? id : null);
    setDropModelId(null);
    setDropGroupId(null);
  };
  const startGroupDrag = (event: React.DragEvent<HTMLTableRowElement>, groupId: string) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", `group:${groupId}`);
    setDragGroupId(groupId);
    setDropGroupId(null);
  };
  const startModelDrag = (event: React.DragEvent<HTMLTableRowElement>, modelId: string) => {
    // Interactive controls inside a draggable row must keep their normal
    // click/focus behavior; the row itself is the drag surface.
    if ((event.target as HTMLElement).closest("button, input, textarea, a")) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", `model:${modelId}`);
    setDragModelId(modelId);
    setDropModelId(null);
  };
  const toggleGroup = (groupId: string) => {
    setCollapsedGroups((current) => ({ ...current, [groupId]: !current[groupId] }));
  };
  if (!models.length) return <div className="model-rules-empty"><EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} /></div>;
  return <><section className="model-rules relay-compact-content" aria-label={t("models.visible")}>
      <div className="relay-table-wrap"><table className="relay-table model-rules-table">
      <colgroup><col data-column="model" /><col data-column="actions" /></colgroup>
      <thead><tr><th>{t("common.model")}</th><th>{t("common.actions")}</th></tr></thead>
      {modelGroups.map((group) => {
      const groupCollapsed = Boolean(collapsedGroups[group.id]);
      const groupLabel = t(`modelGroups.${group.id}`, { defaultValue: group.label });
      return <tbody key={group.id} id={`model-group-${group.id}`}>
      <tr className={`model-group-row${dragGroupId === group.id ? " model-dragging" : ""}`} data-group-id={group.id} data-drop-target={dropGroupId === group.id ? "true" : undefined} draggable onPointerDown={(event) => startPointerDrag(event, "group", group.id)} onDragStart={(event) => startGroupDrag(event, group.id)} onDragEnd={() => { setDragGroupId(null); setDropGroupId(null); }} onDragOver={(event) => { event.preventDefault(); setDropGroupId(dragGroupId && dragGroupId !== group.id ? group.id : null); }} onDrop={() => { if (dragGroupId) reorderGroups(dragGroupId, group.id); setDragGroupId(null); setDropGroupId(null); }}><th colSpan={2} scope="rowgroup"><span className="model-group-content"><button className="model-group-toggle" type="button" aria-expanded={!groupCollapsed} aria-controls={`model-group-${group.id}`} aria-label={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })} data-relay-tooltip={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })} onClick={() => toggleGroup(group.id)}>{groupCollapsed ? <ChevronRight aria-hidden /> : <ChevronDown aria-hidden />}</button><span className="model-group-drag-handle" data-relay-tooltip={t("models.dragGroup", { group: groupLabel })}><GripVertical aria-hidden /></span><strong>{groupLabel}</strong><small>{t("models.groupCount", { count: group.items.length })}</small></span></th></tr>
      {!groupCollapsed && group.items.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const displayName = model.catalogName || model.codexDisplayName || model.id;
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      const hasReasoningModes = (model.reasoningLevels?.length ?? 0) > 0 || (model.reasoningSupportedLevels?.length ?? 0) > 0 || model.reasoningManualFallback === true;
      const canEditReasoning = Boolean(model.reasoningConfigurable);
      const speedTier = model.speedTier ?? "standard";
      const speedTiers = modelSpeedTiers(model, speedTier);
      const canEditSpeed = model.speedSupported === true
        && model.speedConfigurable === true
        && speedTiers.length > 1;
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"} data-drop-target={dropModelId === model.id ? "true" : undefined} className={dragModelId === model.id ? "model-dragging" : undefined} draggable onPointerDown={(event) => startPointerDrag(event, "model", model.id)} onDragStart={(event) => startModelDrag(event, model.id)} onDragEnd={() => { setDragModelId(null); setDropModelId(null); }} onDragOver={(event) => { event.preventDefault(); setDropModelId(dragModelId && dragModelId !== model.id ? model.id : null); }} onDrop={() => { if (dragModelId) reorderModels(dragModelId, model.id); setDragModelId(null); setDropModelId(null); }}>
        <td data-column="model"><button className="model-rule-drag-handle" type="button" aria-label={t("models.dragModel", { model: displayName })} data-relay-tooltip={t("models.dragModel", { model: displayName })} onPointerDown={(event) => startPointerDrag(event, "model", model.id)}><GripVertical aria-hidden /></button><div className="model-rule-identity"><strong data-relay-tooltip={displayName}>{displayName}</strong>{displayName !== model.id ? <code data-relay-tooltip={model.id}>{model.id}</code> : null}</div></td>
                <td data-column="actions"><div className="model-rule-actions"><span className="model-rule-secondary-actions"><IconButton data-model-reasoning-edit={model.id} label={t(canEditReasoning ? "models.editReasoning" : "models.viewReasoning", { model: model.id })} icon={<BrainCircuit aria-hidden />} disabled={!hasReasoningModes} onClick={() => setReasoningModel(model)} />{canEditSpeed ? <span className="model-speed-toggle" data-speed-tier={speedTier} data-model-speed-select={model.id}><OptionMenu className="model-speed-select" label={`${t("pool.serviceTier")}: ${t(`pool.serviceTiers.${speedTier}`)}`} value={speedTier} icon={<Zap aria-hidden />} disabled={busy === `model-speed-${model.id}`} onChange={(value) => { const nextTier = value as DefaultServiceTier; void perform(`model-speed-${model.id}`, () => mode === "local" ? relayCommands.setModelServiceTier(model.id, nextTier) : relayCommands.remoteAction({ type: "set_model_service_tier" }, { modelId: model.id, serviceTier: nextTier }), "feedback.saved"); }} options={speedTiers.map((value) => ({ value, label: t(`pool.serviceTiers.${value}`) }))} /></span> : null}</span><IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
      })}</tbody>;
      })}
    </table></div>
  </section>{reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}</>;
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
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
  const [mutationInFlight, setMutationInFlight] = useState(false);
  const operation = `model-reasoning-${model.id}`;
  const label = (level: string) => t(`usage.reasoningEfforts.${level}`, { defaultValue: formatReasoningEffort(level) });
  const updateAllowedLevels = (next: string[]) => {
    allowedLevelsRef.current = next;
    setAllowedLevels(next);
  };
  const runSerialized = async (id: string, work: () => Promise<unknown>, successKey?: string) => {
    if (mutationLock.current) return false;
    mutationLock.current = true;
    setMutationInFlight(true);
    try {
      return await perform(id, work, successKey);
    } finally {
      mutationLock.current = false;
      setMutationInFlight(false);
    }
  };
  const saveLevels = async (next: string[]) => {
    if (mutationLock.current) return;
    const normalized = normalizeToSupported(next);
    const revision = ++policyRevision.current;
    const ok = await runSerialized(operation, () => mode === "local"
      ? relayCommands.setModelReasoning(model.id, normalized)
      : relayCommands.remoteAction({ type: "set_model_reasoning" }, { modelId: model.id, allowedLevels: normalized }), "feedback.saved");
    if (ok && revision === policyRevision.current) updateAllowedLevels(normalized);
  };
  const toggleAllowedLevel = (level: string) => {
    const next = normalizeToSupported(toggleReasoningLevel(allowedLevelsRef.current, level));
    if (next === allowedLevelsRef.current) return;
    void saveLevels(next);
  };
  const manualBusy = mutationInFlight || busy === operation;
  return <Dialog className="model-reasoning-dialog" title={t("models.reasoningTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <div className="model-reasoning-form">
      <code className="model-reasoning-model" data-relay-tooltip={model.id}>{model.id}</code>
      <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningTitle")}>
        {supportedLevels.map((level) => <button key={level} type="button" role="checkbox" aria-checked={allowedLevels.includes(level)} className={allowedLevels.includes(level) ? "selected" : undefined} disabled={!editable || manualBusy} onClick={() => toggleAllowedLevel(level)}>{label(level)}</button>)}
      </div>
    </div>
  </Dialog>;
}
