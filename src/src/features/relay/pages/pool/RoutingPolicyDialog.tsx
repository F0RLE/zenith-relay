import { useState, type DragEvent, type KeyboardEvent } from "react";
import { ArrowDown, ArrowUp, Cloud, GripVertical, ListOrdered, Repeat2, Sparkles, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { PoolRoutingPolicy } from "../../api/types";
import { Button, Dialog, IconButton, StatusBadge } from "../../components/Ui";
import { compareOperationalStatus, operationalStatusTone } from "../../accountStatus";
import { memberName } from "../../poolHelpers";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayState } from "../../state/RelayStateProvider";
import { poolMembersFromRuntime } from "./poolMembersModel";

const MODES = [
  { value: "smart", icon: Sparkles },
  { value: "in_order", icon: ListOrdered },
  { value: "round_robin", icon: Repeat2 },
] as const;

export function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [initial] = useState(runtime?.gateway.poolRouting);
  const [policy, setPolicy] = useState<PoolRoutingPolicy>(initial ?? { version: 1, mode: "smart", members: [] });
  const [dragged, setDragged] = useState<string | null>(null);
  const members = new Map((runtime ? poolMembersFromRuntime(runtime) : []).map((member) => [`${member.kind}:${member.id}`, member]));
  const manualOrder = policy.mode === "in_order";
  const rows = policy.members.map((rule, index) => ({ rule, index, member: members.get(`${rule.kind}:${rule.id}`) }));
  if (!manualOrder) rows.sort((left, right) => compareOperationalStatus(left.member?.operationalStatus ?? "unavailable", right.member?.operationalStatus ?? "unavailable"));
  const listLabel = t(manualOrder ? "pool.memberOrder" : "pool.rotationMembers");
  const saving = busy === "routing-policy";
  const changedElsewhere = JSON.stringify(initial) !== JSON.stringify(runtime?.gateway.poolRouting);
  const chooseModeWithKeyboard = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const offset = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[event.key];
    if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? MODES.length - 1 : (index + (offset ?? 0) + MODES.length) % MODES.length;
    const option = MODES[next];
    if (!option) return;
    setPolicy((current) => ({ ...current, mode: option.value }));
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]')[next]?.focus();
  };
  const move = (from: number, to: number) => setPolicy((current) => {
    if (saving || current.mode !== "in_order" || from === to || from < 0 || to < 0 || to >= current.members.length) return current;
    const next = [...current.members];
    const [member] = next.splice(from, 1);
    if (!member) return current;
    next.splice(to, 0, member);
    return { ...current, members: next };
  });
  const updateMember = (index: number, field: "weight" | "maxConcurrency", value: number) => {
    const min = field === "weight" ? 1 : 0;
    const max = field === "weight" ? 100 : 1024;
    if (!Number.isFinite(value)) return;
    setPolicy((current) => ({ ...current, members: current.members.map((member, i) => i === index ? { ...member, [field]: Math.min(max, Math.max(min, Math.trunc(value))) } : member) }));
  };
  const save = async () => {
    if (!initial || changedElsewhere || saving) return;
    const ok = await perform("routing-policy", () => persistRoutingPolicy(mode, {
      poolRouting: policy,
      expectedPoolRouting: initial,
      maxRetryCandidates: runtime?.gateway.maxRetryCandidates ?? 3,
      cooldownAfterFailures: runtime?.gateway.cooldownAfterFailures ?? 3,
      keepLastCandidateAvailable: runtime?.gateway.keepLastCandidateAvailable ?? true,
      routingStrategy: runtime?.gateway.routingStrategy ?? "adaptive",
      defaultServiceTier: runtime?.gateway.defaultServiceTier ?? "standard",
      subscriptionPlanOrder: runtime?.gateway.subscriptionPlanOrder ?? [],
    }), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide className="pool-routing-dialog" title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<>
    <Button variant="secondary" disabled={saving} onClick={onClose}>{t("common.cancel")}</Button>
    <Button variant="primary" busy={saving} disabled={!initial || changedElsewhere} onClick={() => void save()}>{t("common.save")}</Button>
  </>}>
    <div className="pool-routing-editor" data-manual-order={manualOrder} aria-busy={saving}>
      <div className="pool-routing-modes" role="radiogroup" aria-label={t("pool.routingStrategy")}>
        {MODES.map(({ value, icon: Icon }, index) => <button key={value} type="button" role="radio" aria-checked={policy.mode === value} tabIndex={policy.mode === value ? 0 : -1} disabled={saving} onKeyDown={(event) => chooseModeWithKeyboard(event, index)} onClick={() => setPolicy((current) => ({ ...current, mode: value }))}>
          <Icon aria-hidden /><span>{t(`pool.rotationModes.${value}`)}</span>
        </button>)}
      </div>
      {changedElsewhere ? <p role="alert" className="form-error">{t("pool.routingChanged")}</p> : null}
      {!initial ? <p role="alert" className="form-error">{t("remote.capabilityUnavailable")}</p> : null}
      <div className="pool-routing-columns" aria-hidden><span>{listLabel}</span>{!manualOrder ? <span>{t("pool.rotationWeight")}</span> : null}<span>{t("pool.rotationConcurrency")}</span>{manualOrder ? <span /> : null}</div>
      <div className="pool-routing-order" role="list" aria-label={listLabel}>
        {rows.map(({ rule, index, member }) => {
          const key = `${rule.kind}:${rule.id}`;
          const label = member ? memberName(member) : rule.id;
          const status = member?.operationalStatus ?? "unavailable";
          const Icon = rule.kind === "account" ? UserRound : Cloud;
          const drop = (event: DragEvent<HTMLDivElement>) => {
            event.preventDefault();
            if (dragged) move(policy.members.findIndex((entry) => `${entry.kind}:${entry.id}` === dragged), index);
            setDragged(null);
          };
          return <div className="pool-routing-member" key={key} role="listitem" data-member-id={key} data-status={status} data-dragging={dragged === key || undefined} onDragOver={manualOrder ? (event) => event.preventDefault() : undefined} onDrop={manualOrder ? drop : undefined}>
            <div className="pool-routing-identity">
              {manualOrder ? <><button type="button" className="pool-routing-handle" disabled={saving} draggable={!saving} aria-label={t("pool.reorderMember", { name: label })} data-relay-tooltip={t("pool.reorderMember", { name: label })} onDragStart={() => setDragged(key)} onDragEnd={() => setDragged(null)}><GripVertical aria-hidden /></button><span className="pool-routing-rank">{index + 1}</span></> : null}
              <Icon aria-hidden />
              <span className="pool-routing-name"><strong>{label}</strong><span className="pool-routing-meta"><small>{t(rule.kind === "account" ? "pool.accountMember" : "pool.apiMember")}</small><StatusBadge status={operationalStatusTone(status)} label={t(`pool.memberStatus.${status}`)} /></span></span>
            </div>
            {!manualOrder ? <label className="pool-routing-number"><span>{t("pool.rotationWeight")}</span><input aria-label={t("pool.memberWeight", { name: label })} type="number" min={1} max={100} disabled={saving} value={rule.weight} onChange={(event) => updateMember(index, "weight", event.currentTarget.valueAsNumber)} /></label> : null}
            <label className="pool-routing-number"><span>{t("pool.rotationConcurrency")}</span><input aria-label={t("pool.memberConcurrency", { name: label })} aria-valuetext={rule.maxConcurrency === 0 ? t("pool.unlimitedConcurrency") : undefined} placeholder={t("pool.unlimitedConcurrency")} type="number" min={1} max={1024} disabled={saving} value={rule.maxConcurrency || ""} onChange={(event) => updateMember(index, "maxConcurrency", event.currentTarget.value === "" ? 0 : event.currentTarget.valueAsNumber)} /></label>
            {manualOrder ? <div className="inline-actions"><IconButton label={t("pool.moveMemberUp", { name: label })} icon={<ArrowUp aria-hidden />} disabled={saving || index === 0} onClick={() => move(index, index - 1)} /><IconButton label={t("pool.moveMemberDown", { name: label })} icon={<ArrowDown aria-hidden />} disabled={saving || index === policy.members.length - 1} onClick={() => move(index, index + 1)} /></div> : null}
          </div>;
        })}
      </div>
    </div>
  </Dialog>;
}
