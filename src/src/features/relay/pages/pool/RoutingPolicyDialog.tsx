import { lazy, Suspense, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { ArrowDown, ArrowUp, CircleHelp, Cloud, GripVertical, ListOrdered, ShieldCheck, Sparkles, UserRound, UsersRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, Dialog, IconButton, StatusBadge, ToggleSwitch } from "../../components/Ui";
import { compareOperationalStatus, operationalStatusTone } from "../../accountStatus";
import { memberName } from "../../poolHelpers";
import { usePointerDragListeners, type PointerDragPosition } from "../../hooks/usePointerDragListeners";
import { useRelayState } from "../../state/RelayStateProvider";
import { poolMembersFromRuntime } from "./poolMembersModel";
import { routingMemberKey } from "./poolRoutingEdits";
import { usePoolRoutingEditor } from "./usePoolRoutingEditor";
import { currentRotationPreset, rotationModeName, usesRequestShares } from "./rotationModes";

const HelpTopicDialog = lazy(async () => ({ default: (await import("../../help/HelpCenter")).HelpTopicDialog }));
const PRESETS = [
  { id: "recommended", icon: Sparkles },
  { id: "even", icon: ListOrdered },
  { id: "accounts_first", icon: UsersRound },
] as const;

const MODES = [
  { modeId: "automatic", policyMode: "automatic", icon: Sparkles },
  { modeId: "roundRobin", policyMode: "round_robin", icon: ListOrdered },
  { modeId: "failover", policyMode: "failover", icon: ShieldCheck },
] as const;

export function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const { policy, edit, saving, errorKey, available, close } = usePoolRoutingEditor(onClose);
  const [helpOpen, setHelpOpen] = useState(false);
  const [dragged, setDragged] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const dragRef = useRef<(PointerDragPosition & { member: string }) | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const members = new Map((runtime ? poolMembersFromRuntime(runtime) : []).map((member) => [`${member.kind}:${member.id}`, member]));
  const manualOrder = policy.mode !== "automatic";
  const modern = policy.version === 3;
  const modes = modern ? MODES : MODES.slice(0, 2);
  const shares = policy.mode !== "smart" && usesRequestShares(policy.mode);
  const selectedMode = policy.mode === "smart" ? "automatic" : rotationModeName(policy.mode);
  const preset = policy.version === 1 ? "custom" : currentRotationPreset(policy);
  const rows = policy.members.map((rule, index) => ({ rule, index, member: members.get(`${rule.kind}:${rule.id}`) }));
  if (!manualOrder) rows.sort((left, right) => compareOperationalStatus(left.member?.operationalStatus ?? "unavailable", right.member?.operationalStatus ?? "unavailable"));
  const listLabel = t(manualOrder ? "pool.memberOrder" : "pool.rotationMembers");
  const chooseModeWithKeyboard = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const offset = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
    if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const nextModeIndex = event.key === "Home" ? 0 : event.key === "End" ? modes.length - 1 : (index + (offset ?? 0) + modes.length) % modes.length;
    const option = modes[nextModeIndex];
    if (!option) return;
    edit({ type: "mode", mode: option.policyMode });
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[nextModeIndex]?.focus();
  };
  const move = (sourceIndex: number, targetIndex: number) => {
    const member = policy.members[sourceIndex];
    const target = policy.members[targetIndex];
    if (!manualOrder || sourceIndex === targetIndex || !member || !target) return;
    edit({ type: "move", member: routingMemberKey(member), target: routingMemberKey(target), placement: sourceIndex < targetIndex ? "after" : "before" });
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
  const updateMember = (memberKey: string, field: "weight" | "maxConcurrency", numericValue: number) => {
    const min = field === "weight" ? 1 : 0;
    const max = field === "weight" ? 100 : 1024;
    if (!Number.isFinite(numericValue)) return;
    edit({ type: "member", member: memberKey, field, value: Math.min(max, Math.max(min, Math.trunc(numericValue))) });
  };
  return <><Dialog wide className="pool-routing-dialog" title={t("pool.routingSettingsTitle")}
    headerActions={<IconButton label={t("pool.routingSettingsHelp")} icon={<CircleHelp aria-hidden />} onClick={() => setHelpOpen(true)} />}
    onClose={() => { if (dragRef.current) clearDrag(); else void close(); }}>
    <div
      className="pool-routing-editor"
      data-presets={modern}
      data-manual-order={manualOrder}
      data-request-shares={shares}
      aria-busy={saving}
      onKeyDown={(event) => {
        if (event.key === "Escape" && dragRef.current) {
          event.preventDefault();
          clearDrag();
        }
      }}
    >
      {modern ? <aside className="pool-routing-presets">
        <div className="pool-routing-preset-heading"><span>{t("pool.rotationPreset")}</span>
          {preset === "custom" ? <span className="pool-routing-custom">{t("pool.rotationPresets.custom")}</span> : null}
        </div>
        <div className="pool-routing-preset-options" role="group" aria-label={t("pool.rotationPreset")}>
          {PRESETS.map(({ id, icon: Icon }) => <Button
            key={id}
            variant="secondary"
            className="pool-routing-preset"
            icon={<Icon aria-hidden />}
            aria-pressed={preset === id}
            disabled={!available}
            onClick={() => edit({ type: "preset", preset: id })}
          >{t(`pool.rotationPresets.${id}`)}</Button>)}
        </div>
      </aside> : null}
      <div className="pool-routing-content">
        <section className="pool-routing-strategy" aria-label={t("pool.routingStrategy")}>
          <h3>{t("pool.routingStrategy")}</h3>
          <div className="pool-routing-modes" data-legacy={!modern} role="radiogroup" aria-label={t("pool.routingStrategy")}>
            {modes.map(({ modeId, policyMode, icon: Icon }, index) => (
              <button
                key={modeId}
                type="button"
                role="radio"
                aria-label={t(`pool.rotationModes.${modeId}`)}
                data-mode={modeId}
                aria-checked={modeId === selectedMode}
                tabIndex={modeId === selectedMode ? 0 : -1}
                disabled={!available}
                onKeyDown={(event) => chooseModeWithKeyboard(event, index)}
                onClick={() => edit({ type: "mode", mode: policyMode })}
              >
                <Icon aria-hidden /><span className="pool-routing-mode-full">{t(`pool.rotationModes.${modeId}`)}</span>
                {modeId === "automatic" ? <span className="pool-routing-mode-compact" aria-hidden>{t("pool.automaticCompact")}</span> : null}
              </button>
            ))}
          </div>
          {modern && selectedMode === "roundRobin" ? <div className="pool-routing-shares">
            <span>{t("pool.weightedRoundRobin")}</span>
            <ToggleSwitch
              label={t("pool.weightedRoundRobin")}
              checked={policy.mode === "weighted_round_robin"}
              disabled={!available}
              onChange={(checked) => edit({ type: "mode", mode: checked ? "weighted_round_robin" : "round_robin" })}
            />
          </div> : null}
        </section>
        {errorKey ? <p role="alert" className="form-error">{t(errorKey)}</p> : null}
        {!available ? <p role="alert" className="form-error">{t("remote.capabilityUnavailable")}</p> : null}
        <section className="pool-routing-members">
          <div className="pool-routing-columns">
            <div className="pool-routing-members-heading"><h3>{listLabel}</h3><span>{rows.length}</span></div>
            {shares ? <span aria-hidden>{t("pool.rotationWeight")}</span> : null}
            <span aria-hidden>{t("pool.rotationConcurrency")}</span>
            {manualOrder ? <span aria-hidden /> : null}
          </div>
          <div className="pool-routing-order" ref={listRef} role="list" aria-label={listLabel}>
            {rows.map(({ rule, index, member }) => {
              const key = `${rule.kind}:${rule.id}`;
              const label = member ? memberName(member) : rule.id;
              const status = member?.operationalStatus ?? "unavailable";
              const Icon = rule.kind === "account" ? UserRound : Cloud;
              return <div
                className="pool-routing-member"
                key={key}
                role="listitem"
                data-member-id={key}
                data-status={status}
                data-dragging={dragged === key || undefined}
                data-drop-target={dropTarget === key || undefined}
              >
                <div className="pool-routing-identity">
                  {manualOrder ? (
                    <>
                      <button
                        type="button"
                        className="pool-routing-handle"
                        disabled={!available}
                        aria-label={t("pool.reorderMember", { name: label })}
                        data-relay-tooltip={t("pool.reorderMember", { name: label })}
                        onPointerDown={(event) => startDrag(event, key)}
                      >
                        <GripVertical aria-hidden />
                      </button>
                      <span className="pool-routing-rank">{index + 1}</span>
                    </>
                  ) : null}
                  <span className="pool-routing-avatar"><Icon aria-hidden /></span>
                  <span className="pool-routing-name">
                    <strong>{label}</strong>
                    <span className="pool-routing-meta">
                      <small>{t(rule.kind === "account" ? "pool.accountMember" : "pool.apiMember")}</small>
                      <StatusBadge status={operationalStatusTone(status)} label={t(`pool.memberStatus.${status}`)} />
                    </span>
                  </span>
                </div>
                {shares ? (
                  <label className="pool-routing-number">
                    <span>{t("pool.rotationWeight")}</span>
                    <input
                      aria-label={t("pool.memberWeight", { name: label })}
                      type="number"
                      disabled={!available}
                      min={1}
                      max={100}
                      value={rule.weight}
                      onChange={(event) => updateMember(key, "weight", event.currentTarget.valueAsNumber)}
                    />
                  </label>
                ) : null}
                <label className="pool-routing-number">
                  <span>{t("pool.rotationConcurrency")}</span>
                  <input
                    aria-label={t("pool.memberConcurrency", { name: label })}
                    aria-valuetext={rule.maxConcurrency === 0 ? t("pool.unlimitedConcurrency") : undefined}
                    placeholder={t("pool.unlimitedConcurrency")}
                    type="number"
                    disabled={!available}
                    min={1}
                    max={1024}
                    value={rule.maxConcurrency || ""}
                    onChange={(event) => updateMember(key, "maxConcurrency", event.currentTarget.value === "" ? 0 : event.currentTarget.valueAsNumber)}
                  />
                </label>
                {manualOrder ? (
                  <div className="inline-actions">
                    <IconButton
                      label={t("pool.moveMemberUp", { name: label })}
                      icon={<ArrowUp aria-hidden />}
                      disabled={!available || index === 0}
                      onClick={() => move(index, index - 1)}
                    />
                    <IconButton
                      label={t("pool.moveMemberDown", { name: label })}
                      icon={<ArrowDown aria-hidden />}
                      disabled={!available || index === policy.members.length - 1}
                      onClick={() => move(index, index + 1)}
                    />
                  </div>
                ) : null}
              </div>;
            })}
          </div>
        </section>
      </div>
    </div>
  </Dialog>
    {helpOpen ? <Suspense fallback={null}><HelpTopicDialog topic="pool-rotation" onClose={() => setHelpOpen(false)} /></Suspense> : null}
  </>;
}
