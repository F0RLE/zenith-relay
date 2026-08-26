import { useEffect, useRef, useState } from "react";
import { BrainCircuit, ChevronDown, ChevronRight, CircleAlert, GripVertical, Loader2, Pencil, Power, RotateCcw, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ModelSummary } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, OptionMenu } from "../../components/Ui";
import { supportsCacheWritePricing } from "../../modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../../modelPricing";
import { groupModelSummariesForRules, modelSummaries } from "../../poolHelpers";
import {
  formatModelPrice,
  formatReasoningEffort,
} from "../../poolFormatting";
import {
  initialReasoningLevels,
  toggleReasoningLevel,
} from "./modelReasoningPolicy";
import { useRelayState } from "../../state/RelayStateProvider";

type ModelDragState = {
  kind: "group" | "model";
  id: string;
  pointerId: number;
  clientX: number;
  clientY: number;
};

export function ModelRulesView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [priceModel, setPriceModel] = useState<ModelSummary | null>(null);
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  const models = runtime ? modelSummaries(runtime) : [];
  const [orderedModels, setOrderedModels] = useState<ModelSummary[]>(models);
  const [dragModelId, setDragModelId] = useState<string | null>(null);
  const [dragGroupId, setDragGroupId] = useState<string | null>(null);
  const [dropModelId, setDropModelId] = useState<string | null>(null);
  const [dropGroupId, setDropGroupId] = useState<string | null>(null);
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const modelDragRef = useRef<ModelDragState | null>(null);
  const modelSignature = models.map((model) => [
    model.id,
    model.enabled,
    model.speedSupported,
    model.speedTier,
    model.speedConfigurable,
    model.codexVisible,
    model.codexDisplayName,
    model.catalogRank,
    model.inputMicroUsdPerMillion,
    model.cachedInputMicroUsdPerMillion,
    model.cacheWrite5mMicroUsdPerMillion,
    model.cacheWrite1hMicroUsdPerMillion,
    model.outputMicroUsdPerMillion,
    model.customPrice,
    model.reasoningLevels?.join(","),
    model.reasoningSupportedLevels?.join(","),
    model.reasoningAllowedLevels?.join(","),
  ].join(":" )).join("\u0000");
  useEffect(() => {
    setOrderedModels(models);
  }, [runtime?.configurationRevision, modelSignature]);
  const modelGroups = groupModelSummariesForRules(orderedModels, runtime?.accounts ?? []);
  // A provider/API source can be temporarily unreachable while the local
  // OAuth pool still has a valid catalog. Keep that source error visible on
  // Connections, but do not present it as a global model outage.
  const hasAccountModelFallback = runtime?.accounts.some((account) =>
    account.enabled
      && account.inPool
      && account.secretAvailable
      && account.models.length > 0
      && account.authState.state !== "requires_reauth"
  ) ?? false;
  const discoveryErrors = runtime
    ? [...new Set([
      ...runtime.accounts
        .filter((account) => account.lastErrorCode?.trim().startsWith("models_"))
        .map((account) => `${account.label}: ${account.lastErrorCode!.trim()}`),
      ...(hasAccountModelFallback ? [] : runtime.sources
        .filter((source) => Boolean(source.lastErrorCode?.trim()))
        .map((source) => `${source.name}: ${source.lastErrorCode!.trim()}`)),
      ...(hasAccountModelFallback ? [] : runtime.warnings
        .filter((warning) => warning.startsWith("model_catalog_refresh_failed:"))
        .map((warning) => warning.replace("model_catalog_refresh_failed:", "model catalog: "))),
    ])]
    : [];
  const catalogRefreshDeferred = !hasAccountModelFallback && (runtime?.warnings.includes("model_catalog_refresh_deferred:codex_running") ?? false);
  const discoveryAlert = discoveryErrors.length || catalogRefreshDeferred
    ? <div className={`model-discovery-alert${catalogRefreshDeferred && !discoveryErrors.length ? " deferred" : ""}`} role={discoveryErrors.length ? "alert" : "status"}><CircleAlert aria-hidden /><span><strong>{discoveryErrors.length ? t("models.discoveryError") : t("models.discoveryDeferred")}</strong><small>{discoveryErrors.length ? t("models.discoveryErrorDetail", { errors: discoveryErrors.join(" · ") }) : t("models.discoveryDeferredDetail")}</small></span></div>
    : null;
  const canEditPrice = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("model_pricing"));
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
      ? relayCommands.setModelDisplayOrder(next.map((model) => model.id))
      : relayCommands.remoteAction({ type: "set_model_order" }, { modelIds: next.map((model) => model.id) }),
    "feedback.saved",
  );
  const reorderModels = (sourceId: string, targetId: string) => {
    if (sourceId === targetId) return;
    const next = [...orderedModels];
    const source = next.findIndex((model) => model.id === sourceId);
    const target = next.findIndex((model) => model.id === targetId);
    if (source < 0 || target < 0) return;
    const [moved] = next.splice(source, 1);
    next.splice(target, 0, moved!);
    setOrderedModels(next);
    void saveModelOrder(next);
  };
  const reorderGroups = (sourceId: string, targetId: string) => {
    if (sourceId === targetId) return;
    const source = modelGroups.findIndex((group) => group.id === sourceId);
    const target = modelGroups.findIndex((group) => group.id === targetId);
    if (source < 0 || target < 0) return;
    const blocks = modelGroups.map((group) => group.items);
    const [moved] = blocks.splice(source, 1);
    blocks.splice(target, 0, moved!);
    const next = blocks.flat();
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
      const targetId = row?.dataset.groupId ?? null;
      setDropGroupId(targetId && targetId !== drag.id ? targetId : null);
      setDropModelId(null);
      return;
    }
    const row = target?.closest<HTMLElement>("[data-model-id]");
    const targetId = row?.dataset.modelId ?? null;
    setDropModelId(targetId && targetId !== drag.id ? targetId : null);
    setDropGroupId(null);
  };
  const finishModelDragAt = (clientX: number, clientY: number) => {
    const drag = modelDragRef.current;
    if (!drag) return;
    const target = document.elementFromPoint(clientX, clientY);
    if (drag.kind === "group") {
      const targetId = target?.closest<HTMLElement>("[data-group-id]")?.dataset.groupId;
      if (targetId && targetId !== drag.id) reorderGroups(drag.id, targetId);
    } else {
      const targetId = target?.closest<HTMLElement>("[data-model-id]")?.dataset.modelId;
      if (targetId && targetId !== drag.id) reorderModels(drag.id, targetId);
    }
    clearModelDrag();
  };
  useEffect(() => {
    const drag = modelDragRef.current;
    if (!drag) return;
    const onPointerMove = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      drag.clientX = event.clientX;
      drag.clientY = event.clientY;
      updateModelDragAt(event.clientX, event.clientY);
    };
    const onPointerUp = (event: globalThis.PointerEvent) => {
      if (event.pointerId === drag.pointerId) finishModelDragAt(event.clientX, event.clientY);
    };
    const onPointerCancel = (event: globalThis.PointerEvent) => {
      if (event.pointerId === drag.pointerId) clearModelDrag();
    };
    const onWheel = () => {
      // Keep normal table/dialog scrolling and refresh the target after it.
      requestAnimationFrame(() => {
        if (modelDragRef.current !== drag) return;
        updateModelDragAt(drag.clientX, drag.clientY);
      });
    };
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", onPointerCancel);
    window.addEventListener("wheel", onWheel, { passive: true });
    return () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      window.removeEventListener("pointercancel", onPointerCancel);
      window.removeEventListener("wheel", onWheel);
    };
  }, [dragModelId, dragGroupId]);
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
  if (!models.length) return <div className="model-rules-empty">{discoveryAlert}<EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} /></div>;
  return <>{discoveryAlert}<section className="model-rules relay-compact-content" aria-label={t("models.visible")}>
    <div className="relay-table-wrap"><table className="relay-table model-rules-table">
      <colgroup><col data-column="model" /><col data-column="price" /><col data-column="speed" /><col data-column="actions" /></colgroup>
      <thead><tr><th>{t("common.model")}</th><th>{t("models.priceColumn")}</th><th>{t("models.speedColumn")}</th><th>{t("common.actions")}</th></tr></thead>
      {modelGroups.map((group) => {
      const groupCollapsed = Boolean(collapsedGroups[group.id]);
      const groupLabel = t(`modelGroups.${group.id}`, { defaultValue: group.label });
      return <tbody key={group.id} id={`model-group-${group.id}`}>
      <tr className={`model-group-row${dragGroupId === group.id ? " model-dragging" : ""}`} data-group-id={group.id} data-drop-target={dropGroupId === group.id ? "true" : undefined} draggable onPointerDown={(event) => startPointerDrag(event, "group", group.id)} onDragStart={(event) => startGroupDrag(event, group.id)} onDragEnd={() => { setDragGroupId(null); setDropGroupId(null); }} onDragOver={(event) => { event.preventDefault(); setDropGroupId(dragGroupId && dragGroupId !== group.id ? group.id : null); }} onDrop={() => { if (dragGroupId) reorderGroups(dragGroupId, group.id); setDragGroupId(null); setDropGroupId(null); }}><th colSpan={4} scope="rowgroup"><span className="model-group-content"><button className="model-group-toggle" type="button" aria-expanded={!groupCollapsed} aria-controls={`model-group-${group.id}`} aria-label={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })} title={t(groupCollapsed ? "models.expandGroup" : "models.collapseGroup", { group: groupLabel })} onClick={() => toggleGroup(group.id)}>{groupCollapsed ? <ChevronRight aria-hidden /> : <ChevronDown aria-hidden />}</button><span className="model-group-drag-handle" title={t("models.dragGroup", { group: groupLabel })}><GripVertical aria-hidden /></span><strong>{groupLabel}</strong><small>{t("models.groupCount", { count: group.items.length })}</small></span></th></tr>
      {!groupCollapsed && group.items.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const imagePrices = model.imageRequestPrices ?? [];
      const isImageModel = imagePrices.length > 0;
      const hasPrice = !isImageModel && model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
      const cachedInputPrice = model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion;
      const priceParts = hasPrice ? [
        { label: t("models.inputPriceLabel"), value: formatModelPrice(model.inputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.outputPriceLabel"), value: formatModelPrice(model.outputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.cachedInputPriceLabel"), value: formatModelPrice(cachedInputPrice!, i18n.language) },
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite5mMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite5mPriceLabel"), value: formatModelPrice(model.cacheWrite5mMicroUsdPerMillion, i18n.language) }] : []),
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite1hMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite1hPriceLabel"), value: formatModelPrice(model.cacheWrite1hMicroUsdPerMillion, i18n.language) }] : []),
      ] : [];
      const displayName = formatModelDisplayName(model.codexDisplayName || model.id);
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      const hasReasoningModes = !isImageModel && ((model.reasoningLevels?.length ?? 0) > 0 || (model.reasoningSupportedLevels?.length ?? 0) > 0);
      const canEditReasoning = !isImageModel && Boolean(model.reasoningConfigurable);
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"} data-drop-target={dropModelId === model.id ? "true" : undefined} className={dragModelId === model.id ? "model-dragging" : undefined} draggable onPointerDown={(event) => startPointerDrag(event, "model", model.id)} onDragStart={(event) => startModelDrag(event, model.id)} onDragEnd={() => { setDragModelId(null); setDropModelId(null); }} onDragOver={(event) => { event.preventDefault(); setDropModelId(dragModelId && dragModelId !== model.id ? model.id : null); }} onDrop={() => { if (dragModelId) reorderModels(dragModelId, model.id); setDragModelId(null); setDropModelId(null); }}>
        <td data-column="model"><button className="model-rule-drag-handle" type="button" aria-label={t("models.dragModel", { model: displayName })} title={t("models.dragModel", { model: displayName })} onPointerDown={(event) => startPointerDrag(event, "model", model.id)}><GripVertical aria-hidden /></button><div className="model-rule-identity"><strong title={displayName}>{displayName}</strong>{displayName !== model.id ? <code title={model.id}>{model.id}</code> : null}</div></td>
        <td data-column="price"><div className="model-price">{isImageModel ? <div className="model-image-price-summary"><span className="model-image-price-heading"><strong>{t("models.imageOperation.generation")}</strong><small>{t("models.imagePriceUnit")}</small></span><div className="model-image-price-list">{imagePrices.filter((price) => price.operation === "generation").slice(0, 3).map((price) => <span className="model-image-price-item" key={`${price.quality}-${price.size}`}><small>{t(`models.imageQuality.${price.quality}`, { defaultValue: price.quality })} · {price.size}</small><strong>{formatModelPrice(price.microUsd, i18n.language)}</strong></span>)}</div></div> : hasPrice ? <>{priceParts.map((part) => <span className="model-price-value" key={part.label}><small>{part.label}</small><strong>{part.value}</strong></span>)}{model.customPrice ? <small className="model-price-note custom">{t("models.customPrice")}</small> : null}</> : <span className="model-price-empty muted">{t("models.priceUnavailable")}</span>}</div></td>
        <td data-column="speed">{model.speedSupported ? <OptionMenu className="model-speed-menu" label={t("models.speedColumn")} value={model.speedTier ?? "standard"} disabled={!model.speedConfigurable || busy === `model-speed-${model.id}`} onChange={(value) => { const serviceTier = value as "standard" | "fast"; void perform(`model-speed-${model.id}`, () => mode === "local" ? relayCommands.setModelServiceTier(model.id, serviceTier) : relayCommands.remoteAction({ type: "set_model_service_tier" }, { modelId: model.id, serviceTier }), "feedback.saved"); }} options={[{ value: "standard", label: t("pool.serviceTiers.standard") }, { value: "fast", label: t("pool.serviceTiers.fast") }]} /> : <span className="model-speed-unavailable"><Zap aria-hidden />—</span>}</td>
        <td data-column="actions"><div className="model-rule-actions"><span className="model-rule-secondary-actions">{(canEditPrice || isImageModel) ? <IconButton data-model-price-edit={model.id} label={t(isImageModel ? "models.viewImagePrice" : "models.editPrice", { model: model.id })} icon={<Pencil aria-hidden />} onClick={() => setPriceModel(model)} /> : null}{canEditReasoning || hasReasoningModes ? <IconButton data-model-reasoning-edit={model.id} label={t(canEditReasoning ? "models.editReasoning" : "models.viewReasoning", { model: model.id })} icon={<BrainCircuit aria-hidden />} onClick={() => setReasoningModel(model)} /> : null}</span><IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
      })}</tbody>;
      })}
    </table></div>
  </section>{priceModel ? <ModelPriceDialog key={priceModel.id} model={priceModel} onClose={() => setPriceModel(null)} /> : null}{reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}</>;
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  // The backend owns the provider contract and its order. Never synthesize
  // or reorder levels in the editor, and discard stale custom values from old
  // local policies before they can be sent back to the runtime.
  const supportedLevels = (model.reasoningSupportedLevels?.length
    ? model.reasoningSupportedLevels
    : model.reasoningLevels ?? [])
    .map((level) => level.trim().toLowerCase())
    .filter((level, index, levels) => Boolean(level) && levels.indexOf(level) === index);
  const editable = Boolean(model.reasoningConfigurable);
  const normalizeToSupported = (levels: string[]) => {
    const selected = new Set(levels.map((level) => level.trim().toLowerCase()));
    return supportedLevels.filter((level) => selected.has(level));
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
  return <Dialog title={t("models.reasoningTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <div className="model-reasoning-form">
      <code className="model-reasoning-model" title={model.id}>{model.id}</code>
      <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningTitle")}>
        {supportedLevels.map((level) => <button key={level} type="button" role="checkbox" aria-checked={allowedLevels.includes(level)} className={allowedLevels.includes(level) ? "selected" : undefined} disabled={!editable || manualBusy} onClick={() => toggleAllowedLevel(level)}>{label(level)}</button>)}
      </div>
    </div>
  </Dialog>;
}

function formatModelDisplayName(value: string) {
  return value
    .replace(/\bgpt\s*/i, "GPT-")
    .replace(/\bclaude\s*/i, "Claude ")
    .replace(/\bgemini\s*/i, "Gemini ")
    .replace(/\bgrok\s*/i, "Grok ")
    .replace(/\b(o\d)\b/i, (_, token: string) => token.toUpperCase())
    .replace(/(\d)\s+(\d)(?=\s*$)/, "$1.$2")
    .replace(/\s{2,}/g, " ")
    .trim();
}

function ModelPriceDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const imagePrices = model.imageRequestPrices ?? [];
  if (imagePrices.length) {
    return <Dialog title={t("models.imagePriceTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
      <div className="relay-form model-price-form"><div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.imagePriceUnit")}</span></div>
        {["generation", "edit"].map((operation) => <section className="model-price-section" key={operation}><h3>{t(`models.imageOperation.${operation}`)}</h3><div className="model-price-grid">{imagePrices.filter((price) => price.operation === operation).map((price) => <div className="model-price-value" key={`${operation}-${price.quality}-${price.size}`}><small>{t(`models.imageQuality.${price.quality}`, { defaultValue: price.quality })} · {price.size}</small><strong>{formatModelPrice(price.microUsd, i18n.language)}</strong></div>)}</div></section>)}
      </div>
    </Dialog>;
  }
  const { mode, perform, busy } = useRelayState();
  const [inputPrice, setInputPrice] = useState(formatEditableModelPrice(model.inputMicroUsdPerMillion));
  const [cachedInputPrice, setCachedInputPrice] = useState(formatEditableModelPrice(model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion));
  const [cacheWrite5mPrice, setCacheWrite5mPrice] = useState(formatEditableModelPrice(model.cacheWrite5mMicroUsdPerMillion ?? null));
  const [cacheWrite1hPrice, setCacheWrite1hPrice] = useState(formatEditableModelPrice(model.cacheWrite1hMicroUsdPerMillion ?? null));
  const [outputPrice, setOutputPrice] = useState(formatEditableModelPrice(model.outputMicroUsdPerMillion));
  const cacheWrite = supportsCacheWritePricing(model.id);
  const inputMicroUsd = parseEditableModelPrice(inputPrice);
  const cachedInputMicroUsd = parseEditableModelPrice(cachedInputPrice);
  const cacheWrite5mMicroUsd = parseOptionalEditableModelPrice(cacheWrite5mPrice);
  const cacheWrite1hMicroUsd = parseOptionalEditableModelPrice(cacheWrite1hPrice);
  const outputMicroUsd = parseEditableModelPrice(outputPrice);
  const operation = `model-price-${model.id}`;
  const valid = inputMicroUsd != null && cachedInputMicroUsd != null && outputMicroUsd != null && cacheWrite5mMicroUsd !== undefined && cacheWrite1hMicroUsd !== undefined;
  const setPrice = (input: number | null, cachedInput: number | null, write5m: number | null, write1h: number | null, output: number | null) => mode === "local"
    ? relayCommands.setModelPrice(model.id, input, cachedInput, write5m, write1h, output)
    : relayCommands.remoteAction({ type: "set_model_price" }, { modelId: model.id, inputMicroUsdPerMillion: input, cachedInputMicroUsdPerMillion: cachedInput, cacheWrite5mMicroUsdPerMillion: write5m, cacheWrite1hMicroUsdPerMillion: write1h, outputMicroUsdPerMillion: output });
  const save = async () => {
    if (!valid) return;
    const ok = await perform(operation, () => setPrice(inputMicroUsd, cachedInputMicroUsd, cacheWrite ? cacheWrite5mMicroUsd ?? null : null, cacheWrite ? cacheWrite1hMicroUsd ?? null : null, outputMicroUsd), "feedback.saved");
    if (ok) onClose();
  };
  const restore = async () => {
    const ok = await perform(operation, () => setPrice(null, null, null, null, null), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("models.priceTitle")} onClose={onClose} footer={<>{model.customPrice ? <Button className="model-price-restore" variant="secondary" icon={<RotateCcw aria-hidden />} disabled={busy === operation} onClick={() => void restore()}>{t("models.restorePrice")}</Button> : null}<Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} disabled={!valid} onClick={() => void save()}>{t("common.save")}</Button></>}>
    <div className="relay-form model-price-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.priceUnit")}</span></div>
      <section className="model-price-section">
        <h3>{t("models.tokenPrices")}</h3>
        <div className="model-price-grid">
          <ModelPriceField label={t("models.inputPriceLabel")} value={inputPrice} invalid={inputPrice.length > 0 && inputMicroUsd == null} onChange={setInputPrice} />
          <ModelPriceField label={t("models.outputPriceLabel")} value={outputPrice} invalid={outputPrice.length > 0 && outputMicroUsd == null} onChange={setOutputPrice} />
          <ModelPriceField label={t("models.cachedInputPriceLabel")} value={cachedInputPrice} invalid={cachedInputPrice.length > 0 && cachedInputMicroUsd == null} onChange={setCachedInputPrice} />
        </div>
      </section>
      {cacheWrite ? <section className="model-price-section">
        <h3>{t("models.cachePrices")}</h3>
        <div className="model-price-grid ttl">
          <ModelPriceField label={t("models.cacheWrite5mPriceLabel")} value={cacheWrite5mPrice} invalid={cacheWrite5mMicroUsd === undefined} onChange={setCacheWrite5mPrice} />
          <ModelPriceField label={t("models.cacheWrite1hPriceLabel")} value={cacheWrite1hPrice} invalid={cacheWrite1hMicroUsd === undefined} onChange={setCacheWrite1hPrice} />
        </div>
      </section> : null}
    </div>
  </Dialog>;
}

function ModelPriceField({ label, value, invalid, onChange }: { label: string; value: string; invalid: boolean; onChange: (value: string) => void }) {
  return <label className="model-price-field"><span className="model-price-label">{label}</span><span className="model-price-input"><span className="model-price-currency" aria-hidden>$</span><input aria-label={label} type="text" inputMode="decimal" autoComplete="off" spellCheck={false} value={value} placeholder="0.00" aria-invalid={invalid || undefined} onChange={(event) => onChange(event.target.value)} /></span></label>;
}
