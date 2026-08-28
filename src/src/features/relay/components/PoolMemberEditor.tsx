import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import { ArrowDown, ArrowRight, ArrowUp, GripVertical, Power } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { SourceSummary } from "../api/types";
import { SourcePriceEditor, parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "./SourcePriceEditor";
import { effectiveSourceProtocolBindings } from "../sourceProtocolBindings";
import { Button, Dialog, IconButton, OptionMenu, StatusIcon } from "./Ui";
import { apiSourcePriority, apiSourceRole, type ApiSourceRole } from "../routingOrder";
import { sourceOrderForRole, sourceRoutingStages, toggle, type PoolMember } from "../poolHelpers";
import { useRelayState } from "../state/RelayStateProvider";
import {
  modelSelectionForMember,
  modelSelectionPayload,
  moveSourceBy as moveSourceByOrder,
  moveSourceOrder,
  sourcePrioritiesForOrder,
} from "./poolMemberEditorModel";

export function PoolMemberEditor({ member, onClose }: { member: PoolMember; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [sourceRole, setSourceRole] = useState<ApiSourceRole>(apiSourceRole(member.priority));
  const [sourceOrder, setSourceOrder] = useState<string[]>(() => member.kind === "source" ? sourceOrderForRole(runtime?.sources ?? [], apiSourceRole(member.priority), member.id) : []);
  const [draggedSource, setDraggedSource] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [dropAfter, setDropAfter] = useState(false);
  const [dropRole, setDropRole] = useState<ApiSourceRole | null>(null);
  const sourceDragRef = useRef<{ pointerId: number; sourceId: string; clientX: number; clientY: number } | null>(null);
  const [recoveryDelaySeconds, setRecoveryDelaySeconds] = useState(member.kind === "source" ? member.recoveryDelaySeconds ?? 0 : 0);
  const { modelIds, enabledModels: initialEnabledModels } = modelSelectionForMember(member);
  const [enabledModels, setEnabledModels] = useState(initialEnabledModels);
  const toggleEnabledModel = useCallback((model: string) => {
    setEnabledModels((values) => toggle(values, model));
  }, []);
  const [draining, setDraining] = useState(member.draining);
  const [sourcePriceDraftsState, setSourcePriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(member.kind === "source" ? member.modelPriceOverrides ?? {} : {}));
  const sourcePriceOverrides = useMemo(() => parseSourcePriceDrafts(sourcePriceDraftsState), [sourcePriceDraftsState]);
  const [purchaseCost, setPurchaseCost] = useState(member.kind === "account" && member.purchaseCostMicroUsd ? String(member.purchaseCostMicroUsd / 1_000_000) : "");
  const purchaseCostUsd = purchaseCost.trim() === "" ? 0 : Number(purchaseCost);
  const purchaseCostValid = Number.isFinite(purchaseCostUsd) && purchaseCostUsd >= 0 && purchaseCostUsd <= 1_000_000;
  const sourceStages = member.kind === "source" ? sourceRoutingStages(runtime?.sources ?? [], runtime?.accounts ?? [], member.id, sourceRole) : [];
  const orderedSources = sourceOrder.map((sourceId) => runtime?.sources.find((source) => source.id === sourceId)).filter((source): source is SourceSummary => Boolean(source));
  const clearSourceDrag = useCallback(() => {
    sourceDragRef.current = null;
    setDraggedSource(null);
    setDropTarget(null);
    setDropAfter(false);
    setDropRole(null);
  }, []);
  const chooseSourceRole = useCallback((role: ApiSourceRole) => {
    setSourceRole(role);
    setSourceOrder(sourceOrderForRole(runtime?.sources ?? [], role, member.id));
    clearSourceDrag();
  }, [clearSourceDrag, member.id, runtime?.sources]);
  const moveSource = useCallback((sourceId: string, targetId: string, after = false) => {
    setSourceOrder((current) => moveSourceOrder(current, sourceId, targetId, after));
  }, []);
  const moveSourceBy = (sourceId: string, offset: number) => {
    setSourceOrder((current) => moveSourceByOrder(current, sourceId, offset));
  };
  const updateSourceDragAt = useCallback((clientX: number, clientY: number) => {
    const sourceId = sourceDragRef.current?.sourceId;
    if (!sourceId) return;
    const target = document.elementFromPoint(clientX, clientY);
    const role = sourceId === member.id ? sourceRoleAt(target) : null;
    if (role) {
      setDropRole(role);
      setDropTarget(null);
      setDropAfter(false);
      return;
    }
    const row = target?.closest<HTMLElement>("[data-source-id]");
    const targetId = row?.dataset.sourceId ?? null;
    setDropRole(null);
    setDropTarget(targetId && targetId !== sourceId ? targetId : null);
    if (targetId && targetId !== sourceId && row) {
      const bounds = row.getBoundingClientRect();
      setDropAfter(clientY >= bounds.top + bounds.height / 2);
    } else {
    setDropAfter(false);
    }
  }, [member.id]);
  const finishSourceDragAt = useCallback((clientX: number, clientY: number) => {
    const sourceId = sourceDragRef.current?.sourceId;
    if (!sourceId) return;
    const target = document.elementFromPoint(clientX, clientY);
    const role = sourceId === member.id ? sourceRoleAt(target) : null;
    if (role) {
      chooseSourceRole(role);
      return;
    }
    const row = target?.closest<HTMLElement>("[data-source-id]");
    const targetId = row?.dataset.sourceId;
    if (targetId && targetId !== sourceId && row) {
      const bounds = row.getBoundingClientRect();
      moveSource(sourceId, targetId, clientY >= bounds.top + bounds.height / 2);
    }
    clearSourceDrag();
  }, [clearSourceDrag, member.id, moveSource, chooseSourceRole]);
  useEffect(() => {
    const drag = sourceDragRef.current;
    if (!drag || draggedSource !== drag.sourceId) return;
    const onPointerMove = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      drag.clientX = event.clientX;
      drag.clientY = event.clientY;
      updateSourceDragAt(event.clientX, event.clientY);
    };
    const onPointerUp = (event: globalThis.PointerEvent) => {
      if (event.pointerId !== drag.pointerId) return;
      finishSourceDragAt(event.clientX, event.clientY);
    };
    const onPointerCancel = (event: globalThis.PointerEvent) => {
      if (event.pointerId === drag.pointerId) clearSourceDrag();
    };
    const onWheel = () => {
      // Let the dialog's scroll container handle the wheel normally. Re-check
      // the target after scrolling so the drop indicator follows the row.
      requestAnimationFrame(() => {
        if (sourceDragRef.current !== drag) return;
        updateSourceDragAt(drag.clientX, drag.clientY);
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
  }, [clearSourceDrag, draggedSource, finishSourceDragAt, updateSourceDragAt]);
  const startSourceDrag = (event: PointerEvent<HTMLButtonElement>, sourceId: string) => {
    if (event.button !== 0) return;
    event.preventDefault();
    sourceDragRef.current = { pointerId: event.pointerId, sourceId, clientX: event.clientX, clientY: event.clientY };
    setDraggedSource(sourceId);
    setDropTarget(null);
    setDropAfter(false);
    setDropRole(null);
  };
  const save = () => {
    if (member.kind === "source" && !sourcePriceOverrides) return;
    const { allowedModels, excludedModels } = modelSelectionPayload(modelIds, enabledModels);
    const persist = () => {
      if (member.kind === "account") {
        const payload = { allowedModels, excludedModels, draining, purchaseCostMicroUsd: Math.round(purchaseCostUsd * 1_000_000) };
        return mode === "local"
          ? relayCommands.updateAccount({ accountId: member.id, ...payload })
          : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      }
      const protocolBindings = effectiveSourceProtocolBindings(member);
      const sourcePriorities = sourcePrioritiesForOrder(sourceOrder, sourceRole);
      const priority = sourcePriorities[member.id] ?? apiSourcePriority(sourceRole);
      const payload = { allowedModels, excludedModels, draining: member.draining, priority, sourcePriorities, weight: 1, recoveryDelaySeconds, modelPriceOverrides: sourcePriceOverrides ?? {}, protocolBindings };
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, ...payload };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    };
    onClose();
    void perform(`member-${member.id}`, persist, "feedback.saved");
  };
  return <Dialog wide className={member.kind === "source" ? "source-policy-dialog" : ""} title={`${t("pool.editMember")} · ${member.kind === "source" ? member.name : member.label}`} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave || !purchaseCostValid || (member.kind === "source" && !sourcePriceOverrides)} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={save}>{t("pool.savePolicy")}</Button></>}>
    <div className="relay-form member-editor">
      {member.kind === "source" ? <section className="source-routing-section">
        <header className="source-routing-heading"><div><h3>{t("sources.poolRole")}</h3><p className="sr-only">{t("sources.routingHint")}</p></div></header>
        <div className="source-route-order" role="group" aria-label={t("sources.fallbackOrder")}>
          <span>{t("sources.fallbackOrder")}</span>
          <div className="source-route-map" role="radiogroup" aria-label={t("sources.poolRole")} data-dragging={draggedSource ? "true" : undefined}>
            {sourceStages.map((stage, index) => {
              const label = stage.role === "accounts" ? t("connections.accounts") : t(`sources.roles.${stage.role}`);
              const arrow = index > 0 ? <ArrowRight className="source-route-arrow" aria-hidden /> : null;
              if (stage.role === "accounts") {
                return <Fragment key={stage.role}>{arrow}<div className="source-route-stage accounts" data-stage-index={index} aria-label={`${label}: ${stage.count}`}><small>{index + 1}</small><strong>{label}</strong><span>{stage.count}</span></div></Fragment>;
              }
              const role: ApiSourceRole = stage.role;
              return <Fragment key={role}>{arrow}<button className="source-route-stage" data-stage-index={index} data-current={role === sourceRole ? "true" : undefined} data-source-role={role} data-drop-available={draggedSource === member.id ? "true" : undefined} data-drop-target={dropRole === role ? "true" : undefined} type="button" role="radio" aria-checked={role === sourceRole} aria-label={`${label}: ${t(`sources.roleHints.${role}`)}`} onClick={() => chooseSourceRole(role)}><small>{index + 1}</small><strong>{label}</strong><span>{stage.count}</span></button></Fragment>;
            })}
          </div>
        </div>
        <p className="source-role-help">{t(`sources.roleHints.${sourceRole}`)}</p>
        <div className="source-priority-policy">
          <header title={t("sources.sourceOrderHint")}><strong>{t("sources.sourceOrder")}</strong><small className="sr-only">{t("sources.sourceOrderHint")}</small></header>
          <div className="subscription-plan-order source-priority-order" role="list" aria-label={t("sources.sourceOrder")}>{orderedSources.map((source, index) => {
            return <div key={source.id} className="subscription-plan-order-row source-priority-row" role="listitem" data-source-id={source.id} data-current={source.id === member.id ? "true" : undefined} data-dragging={draggedSource === source.id ? "true" : undefined} data-drop-target={dropTarget === source.id ? "true" : undefined} data-drop-after={dropTarget === source.id && dropAfter ? "true" : undefined}>
              <button className="source-priority-drag-handle" data-source-drag-handle type="button" aria-label={t("sources.reorderSource", { source: source.name })} title={t("sources.reorderSource", { source: source.name })} onPointerDown={(event) => startSourceDrag(event, source.id)}><GripVertical aria-hidden /></button>
              <span className="subscription-plan-rank">{index + 1}</span>
              <span className="source-priority-name"><strong>{source.name}</strong>{source.id === member.id ? <small>{t("sources.currentSource")}</small> : null}</span>
              <div className="inline-actions"><IconButton label={t("sources.moveSourceUp", { source: source.name })} icon={<ArrowUp aria-hidden />} disabled={index === 0} onClick={() => moveSourceBy(source.id, -1)} /><IconButton label={t("sources.moveSourceDown", { source: source.name })} icon={<ArrowDown aria-hidden />} disabled={index === orderedSources.length - 1} onClick={() => moveSourceBy(source.id, 1)} /></div>
            </div>;
          })}</div>
        </div>
        <div className="source-routing-controls">
          <div className="relay-field source-routing-control"><span>{t("sources.recoveryDelay")}</span><OptionMenu className="field-option-menu" label={t("sources.recoveryDelay")} value={String(recoveryDelaySeconds)} onChange={(value) => setRecoveryDelaySeconds(Number(value))} options={[0, 5, 30, 60, 300, 900].map((seconds) => ({ value: String(seconds), label: seconds === 0 ? t("sources.recoveryAutomatic") : formatRecoveryDelay(seconds, t) }))} /><small className="sr-only">{t("sources.recoveryDelayHint")}</small></div>
        </div>
      </section> : <><div className="settings-row"><label className="toggle-row"><input type="checkbox" checked={draining} onChange={(event) => setDraining(event.target.checked)} /><span>{t("accounts.drain")}</span></label></div><label className="relay-field"><span>{t("accounts.accountValue.purchaseCost")}</span><input type="number" min="0" max="1000000" step="0.01" value={purchaseCost} onChange={(event) => setPurchaseCost(event.target.value)} placeholder="0.00" /><small>{t("accounts.accountValue.purchaseCostHint")}</small></label></>}
      {member.kind === "source" ? <SourcePriceEditor source={member} drafts={sourcePriceDraftsState} onChange={setSourcePriceDrafts} enabledModels={enabledModels} onToggleModel={toggleEnabledModel} /> : <details className="member-model-rules source-editor-panel">
        <summary className="source-editor-panel-summary"><span><strong>{t("common.models")}</strong><small>{t("models.memberRulesHint")}</small></span><small>{t("common.enabled")}: {enabledModels.length}/{modelIds.length}</small></summary>
        <div className="member-model-content">{modelIds.length ? <ul>{modelIds.map((model) => {
          const enabled = enabledModels.includes(model);
          return <li key={model} data-member-model-id={model} data-enabled={enabled ? "true" : "false"}>
            <code>{model}</code>
            <StatusIcon status={enabled ? "ready" : "disabled"} label={t(enabled ? "models.available" : "models.disabled")} />
            <IconButton className="member-model-toggle" aria-pressed={enabled} label={t(enabled ? "models.disable" : "models.enable", { model })} icon={<Power aria-hidden />} onClick={() => toggleEnabledModel(model)} />
          </li>;
        })}</ul> : <p className="form-note">{t("models.emptyDescription")}</p>}</div>
      </details>}
    </div>
  </Dialog>;
}

function formatRecoveryDelay(seconds: number, t: TFunction) {
  return seconds < 60 ? t("sources.recoverySeconds", { count: seconds }) : t("sources.recoveryMinutes", { count: seconds / 60 });
}

function sourceRoleAt(target: Element | null): ApiSourceRole | null {
  const role = target?.closest<HTMLElement>("[data-source-role]")?.dataset.sourceRole;
  return role === "primary" || role === "stabilizer" || role === "reserve" ? role : null;
}
