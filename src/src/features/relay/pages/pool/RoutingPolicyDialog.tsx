import { useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { ArrowDown, ArrowUp, Cloud, GripVertical, ListOrdered, Repeat2, Sparkles, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, Dialog, IconButton, StatusBadge } from "../../components/Ui";
import { compareOperationalStatus, operationalStatusTone } from "../../accountStatus";
import { memberName } from "../../poolHelpers";
import { usePointerDragListeners, type PointerDragPosition } from "../../hooks/usePointerDragListeners";
import { useRelayState } from "../../state/RelayStateProvider";
import { poolMembersFromRuntime } from "./poolMembersModel";
import { routingMemberKey } from "./poolRoutingEdits";
import { usePoolRoutingEditor } from "./usePoolRoutingEditor";

const MODES = [
  { value: "automatic", icon: Sparkles },
  { value: "in_order", icon: ListOrdered },
  { value: "round_robin", icon: Repeat2 },
] as const;

export function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const { policy, edit, saving, errorKey, available, close } = usePoolRoutingEditor(onClose);
  const [dragged, setDragged] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const dragRef = useRef<(PointerDragPosition & { member: string }) | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const members = new Map((runtime ? poolMembersFromRuntime(runtime) : []).map((member) => [`${member.kind}:${member.id}`, member]));
  const manualOrder = policy.mode === "in_order";
  const rows = policy.members.map((rule, index) => ({ rule, index, member: members.get(`${rule.kind}:${rule.id}`) }));
  if (!manualOrder) rows.sort((left, right) => compareOperationalStatus(left.member?.operationalStatus ?? "unavailable", right.member?.operationalStatus ?? "unavailable"));
  const listLabel = t(manualOrder ? "pool.memberOrder" : "pool.rotationMembers");
  const chooseModeWithKeyboard = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const offset = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
    if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? MODES.length - 1 : (index + (offset ?? 0) + MODES.length) % MODES.length;
    const option = MODES[next];
    if (!option) return;
    edit({ type: "mode", mode: option.value });
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[next]?.focus();
  };
  const move = (from: number, to: number) => {
    const member = policy.members[from];
    const target = policy.members[to];
    if (!manualOrder || from === to || !member || !target) return;
    edit({ type: "move", member: routingMemberKey(member), target: routingMemberKey(target), placement: from < to ? "after" : "before" });
  };
  const clearDrag = () => { dragRef.current = null; setDragged(null); setDropTarget(null); };
  const targetAt = (x: number, y: number) => {
    const row = document.elementFromPoint(x, y)?.closest<HTMLElement>(".pool-routing-member");
    return row && listRef.current?.contains(row) ? row.dataset["memberId"] : undefined;
  };
  usePointerDragListeners({
    dragRef,
    activeKey: dragged,
    onMove: (drag, x, y) => { const target = targetAt(x, y); setDropTarget(target && target !== drag.member ? target : null); },
    onDrop: (drag, x, y) => {
      const target = targetAt(x, y);
      move(policy.members.findIndex((member) => routingMemberKey(member) === drag.member), policy.members.findIndex((member) => routingMemberKey(member) === target));
      clearDrag();
    },
    onCancel: clearDrag,
  });
  const startDrag = (event: PointerEvent<HTMLButtonElement>, member: string) => {
    if (event.button !== 0 || !manualOrder) return;
    event.preventDefault();
    event.currentTarget.focus();
    dragRef.current = { member, pointerId: event.pointerId, clientX: event.clientX, clientY: event.clientY };
    setDragged(member);
  };
  const updateMember = (member: string, field: "weight" | "maxConcurrency", value: number) => {
    const min = field === "weight" ? 1 : 0;
    const max = field === "weight" ? 100 : 1024;
    if (!Number.isFinite(value)) return;
    edit({ type: "member", member, field, value: Math.min(max, Math.max(min, Math.trunc(value))) });
  };
  return <Dialog wide className="pool-routing-dialog" title={t("pool.routingSettingsTitle")} onClose={() => { if (dragRef.current) clearDrag(); else void close(); }} footer={
    <Button variant="secondary" busy={saving} onClick={() => void close()}>{t("common.close")}</Button>
  }>
    <div className="pool-routing-editor" data-manual-order={manualOrder} aria-busy={saving} onKeyDown={(event) => { if (event.key === "Escape" && dragRef.current) { event.preventDefault(); clearDrag(); } }}>
      <div className="pool-routing-modes" role="radiogroup" aria-label={t("pool.routingStrategy")}>
        {MODES.map(({ value, icon: Icon }, index) => <button key={value} type="button" role="radio" aria-checked={policy.mode === value} tabIndex={policy.mode === value ? 0 : -1} disabled={!available} onKeyDown={(event) => chooseModeWithKeyboard(event, index)} onClick={() => edit({ type: "mode", mode: value })}>
          <Icon aria-hidden /><span>{t(`pool.rotationModes.${value}`)}</span>
        </button>)}
      </div>
      {errorKey ? <p role="alert" className="form-error">{t(errorKey)}</p> : null}
      {!runtime?.capabilities.features.includes("rotation_v2") ? <p role="alert" className="form-error">{t("remote.capabilityUnavailable")}</p> : null}
      <div className="pool-routing-columns" aria-hidden><span>{listLabel}</span>{!manualOrder ? <span>{t("pool.rotationWeight")}</span> : null}<span>{t("pool.rotationConcurrency")}</span>{manualOrder ? <span /> : null}</div>
      <div className="pool-routing-order" ref={listRef} role="list" aria-label={listLabel}>
        {rows.map(({ rule, index, member }) => {
          const key = `${rule.kind}:${rule.id}`;
          const label = member ? memberName(member) : rule.id;
          const status = member?.operationalStatus ?? "unavailable";
          const Icon = rule.kind === "account" ? UserRound : Cloud;
          return <div className="pool-routing-member" key={key} role="listitem" data-member-id={key} data-status={status} data-dragging={dragged === key || undefined} data-drop-target={dropTarget === key || undefined}>
            <div className="pool-routing-identity">
              {manualOrder ? <><button type="button" className="pool-routing-handle" disabled={!available} aria-label={t("pool.reorderMember", { name: label })} data-relay-tooltip={t("pool.reorderMember", { name: label })} onPointerDown={(event) => startDrag(event, key)}><GripVertical aria-hidden /></button><span className="pool-routing-rank">{index + 1}</span></> : null}
              <Icon aria-hidden />
              <span className="pool-routing-name"><strong>{label}</strong><span className="pool-routing-meta"><small>{t(rule.kind === "account" ? "pool.accountMember" : "pool.apiMember")}</small><StatusBadge status={operationalStatusTone(status)} label={t(`pool.memberStatus.${status}`)} /></span></span>
            </div>
            {!manualOrder ? <label className="pool-routing-number"><span>{t("pool.rotationWeight")}</span><input aria-label={t("pool.memberWeight", { name: label })} type="number" disabled={!available} min={1} max={100} value={rule.weight} onChange={(event) => updateMember(key, "weight", event.currentTarget.valueAsNumber)} /></label> : null}
            <label className="pool-routing-number"><span>{t("pool.rotationConcurrency")}</span><input aria-label={t("pool.memberConcurrency", { name: label })} aria-valuetext={rule.maxConcurrency === 0 ? t("pool.unlimitedConcurrency") : undefined} placeholder={t("pool.unlimitedConcurrency")} type="number" disabled={!available} min={1} max={1024} value={rule.maxConcurrency || ""} onChange={(event) => updateMember(key, "maxConcurrency", event.currentTarget.value === "" ? 0 : event.currentTarget.valueAsNumber)} /></label>
            {manualOrder ? <div className="inline-actions"><IconButton label={t("pool.moveMemberUp", { name: label })} icon={<ArrowUp aria-hidden />} disabled={!available || index === 0} onClick={() => move(index, index - 1)} /><IconButton label={t("pool.moveMemberDown", { name: label })} icon={<ArrowDown aria-hidden />} disabled={!available || index === policy.members.length - 1} onClick={() => move(index, index + 1)} /></div> : null}
          </div>;
        })}
      </div>
    </div>
  </Dialog>;
}
