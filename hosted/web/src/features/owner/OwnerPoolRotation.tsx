import { useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import { ArrowDown, ArrowUp, Cloud, GripVertical, ListOrdered, ShieldCheck, Sparkles, UserRound, Users } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { OwnerConnection } from "./model";
import { applyRotationPreset, currentRotationPreset, movePoolRotationMember, poolRotationKey, rotationModeName, updatePoolRotationMember, usesRequestShares } from "./poolRotation";
import type { PoolRoutingPolicy } from "./poolRotation";
import "./owner-pool-rotation.css";

const PRESETS = [
  { id: "recommended", label: "pool.rotation.presets.recommended", icon: Sparkles },
  { id: "even", label: "pool.rotation.presets.even", icon: ListOrdered },
  { id: "accounts_first", label: "pool.rotation.presets.accountsFirst", icon: Users },
] as const;
const MODES = [
  { id: "automatic", value: "automatic", icon: Sparkles },
  { id: "roundRobin", value: "round_robin", icon: ListOrdered },
  { id: "failover", value: "failover", icon: ShieldCheck },
] as const;

export function OwnerPoolRotation({ policy, connections, onChange, embedded = false }: {
  policy: PoolRoutingPolicy;
  connections: readonly OwnerConnection[];
  onChange: (policy: PoolRoutingPolicy) => void;
  embedded?: boolean;
}) {
  const { t } = useLocale();
  const [dragged, setDragged] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const manualOrder = policy.mode !== "automatic";
  const shares = usesRequestShares(policy.mode);
  const mode = rotationModeName(policy.mode);
  const preset = currentRotationPreset(policy);
  const inventory = new Map(connections.map(connection => [poolRotationKey({ id: connection.id, kind: connection.kind === "api" ? "source" : "account" }), connection]));
  const rows = policy.members.map((rule, index) => ({ rule, index, member: inventory.get(poolRotationKey(rule)) }));
  if (!manualOrder) rows.sort((a, b) => Number(a.member?.paused ?? true) - Number(b.member?.paused ?? true));
  const listLabel = t(manualOrder ? "pool.rotation.order" : "pool.members");
  const clearDrag = () => { setDragged(null); setDropTarget(null); };
  const targetAt = (event: PointerEvent<HTMLButtonElement>) => {
    const row = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>(".owner-rotation-member");
    return row && listRef.current?.contains(row) ? row.dataset["memberId"] : undefined;
  };
  const chooseMode = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const offset = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
    if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? MODES.length - 1 : (index + (offset ?? 0) + MODES.length) % MODES.length;
    onChange({ ...policy, mode: MODES[next]!.value });
    clearDrag();
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[next]?.focus();
  };

  return <section className="owner-pool-rotation" aria-label={t("pool.rotation.title")} onKeyDown={event => {
    if (event.key === "Escape" && dragged) { event.preventDefault(); clearDrag(); }
  }}>
    {!embedded && <header className="owner-rotation-header"><h2>{t("pool.rotation.title")}</h2></header>}
    <div className="owner-rotation-editor" data-manual-order={manualOrder} data-request-shares={shares}>
      <aside className="owner-rotation-presets">
        <div className="owner-rotation-preset-heading"><span>{t("pool.rotation.preset")}</span>{preset === "custom" && <span className="owner-rotation-custom">{t("pool.rotation.presets.custom")}</span>}</div>
        <div className="owner-rotation-preset-options" role="group" aria-label={t("pool.rotation.preset")}>
          {PRESETS.map(({ id, label, icon: Icon }) => <button key={id} type="button" className="owner-rotation-preset" aria-pressed={preset === id} onClick={() => { clearDrag(); onChange(applyRotationPreset(policy, id)); }}><Icon /><span>{t(label)}</span></button>)}
        </div>
      </aside>
      <div className="owner-rotation-content">
        <section className="owner-rotation-strategy" aria-label={t("pool.rotation.strategy")}>
          <h3>{t("pool.rotation.strategy")}</h3>
          <div className="owner-rotation-modes" role="radiogroup" aria-label={t("pool.rotation.strategy")}>
            {MODES.map(({ id, value, icon: Icon }, index) => <button key={id} type="button" role="radio" data-mode={id} aria-label={t(`pool.rotation.modes.${id}`)} aria-checked={mode === id} tabIndex={mode === id ? 0 : -1} onKeyDown={event => chooseMode(event, index)} onClick={() => { clearDrag(); onChange({ ...policy, mode: value }); }}>
              <Icon /><span className="owner-rotation-mode-full">{t(`pool.rotation.modes.${id}`)}</span>{id === "automatic" && <span className="owner-rotation-mode-compact" aria-hidden="true">{t("pool.rotation.auto")}</span>}
            </button>)}
          </div>
          {mode === "roundRobin" && <label className="owner-rotation-shares"><span>{t("pool.rotation.weighted")}</span><input type="checkbox" role="switch" aria-label={t("pool.rotation.weighted")} checked={policy.mode === "weighted_round_robin"} onChange={event => onChange({ ...policy, mode: event.currentTarget.checked ? "weighted_round_robin" : "round_robin" })} /></label>}
        </section>
        <section className="owner-rotation-members">
          <div className="owner-rotation-columns" aria-hidden="true">
            <div className="owner-rotation-members-heading"><h3>{listLabel}</h3><span>{rows.length}</span></div>
            {shares && <span>{t("pool.rotation.weight")}</span>}<span>{t("pool.rotation.concurrency")}</span>{manualOrder && <span />}
          </div>
          <div className="owner-rotation-order" ref={listRef} role="list" aria-label={listLabel}>
            {rows.map(({ rule, index, member }) => {
              const key = poolRotationKey(rule);
              const name = member?.name ?? rule.id;
              const Icon = rule.kind === "account" ? UserRound : Cloud;
              return <div key={key} role="listitem" className="owner-rotation-member" data-member-id={key} data-dragging={dragged === key || undefined} data-drop-target={dropTarget === key || undefined}>
                <div className="owner-rotation-identity">
                  {manualOrder && <><button type="button" className="owner-rotation-handle" aria-label={t("pool.rotation.reorder", { name })} data-relay-tooltip={t("pool.rotation.reorder", { name })} onPointerDown={event => {
                    if (event.button !== 0) return;
                    event.preventDefault(); event.currentTarget.focus(); event.currentTarget.setPointerCapture(event.pointerId); setDragged(key);
                  }} onPointerMove={event => { if (dragged !== key) return; const target = targetAt(event); setDropTarget(target && target !== key ? target : null); }} onPointerUp={event => {
                    if (dragged === key) { const target = targetAt(event); onChange(movePoolRotationMember(policy, index, policy.members.findIndex(item => poolRotationKey(item) === target))); }
                    clearDrag(); if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
                  }} onPointerCancel={clearDrag} onLostPointerCapture={clearDrag}><GripVertical /></button><span className="owner-rotation-rank">{index + 1}</span></>}
                  <span className="owner-rotation-avatar"><Icon /></span><span className="owner-rotation-name"><strong>{name}</strong><span className="owner-rotation-meta"><small>{t(rule.kind === "account" ? "pool.rotation.account" : "pool.rotation.api")}</small><span className="owner-rotation-state" data-paused={member?.paused ?? true}>{t(member?.paused ? "connections.paused" : "pool.enabled")}</span></span></span>
                </div>
                {shares && <label className="owner-rotation-number"><span>{t("pool.rotation.weight")}</span><input type="number" min={1} max={100} aria-label={t("pool.rotation.memberWeight", { name })} value={rule.weight} onChange={event => onChange(updatePoolRotationMember(policy, key, "weight", event.currentTarget.valueAsNumber))} /></label>}
                <label className="owner-rotation-number"><span>{t("pool.rotation.concurrency")}</span><input type="number" min={1} max={1024} aria-label={t("pool.rotation.memberConcurrency", { name })} aria-valuetext={rule.maxConcurrency === 0 ? t("pool.rotation.unlimited") : undefined} placeholder={t("pool.rotation.unlimited")} value={rule.maxConcurrency || ""} onChange={event => onChange(updatePoolRotationMember(policy, key, "maxConcurrency", event.currentTarget.value === "" ? 0 : event.currentTarget.valueAsNumber))} /></label>
                {manualOrder && <div className="owner-rotation-order-actions">{([{ direction: -1, icon: ArrowUp, label: "pool.rotation.moveUp" }, { direction: 1, icon: ArrowDown, label: "pool.rotation.moveDown" }] as const).map(({ direction, icon: MoveIcon, label }) => <button key={direction} type="button" aria-label={t(label, { name })} data-relay-tooltip={t(label, { name })} disabled={index + direction < 0 || index + direction >= policy.members.length} onClick={() => onChange(movePoolRotationMember(policy, index, index + direction))}><MoveIcon /></button>)}</div>}
              </div>;
            })}
          </div>
          {!rows.length && <p className="owner-rotation-empty" role="status">{t("pool.empty")}</p>}
        </section>
      </div>
    </div>
  </section>;
}
