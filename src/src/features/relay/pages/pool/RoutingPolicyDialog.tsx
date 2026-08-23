import { useState, type DragEvent } from "react";
import { ArrowDown, ArrowUp, GripVertical, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { RoutingStrategy } from "../../api/types";
import { AccountPlanBadge, Button, Dialog, IconButton, OptionMenu } from "../../components/Ui";
import { clampRoutingCount, mergeSubscriptionPlanOrder, subscriptionPlanGroups } from "../../poolHelpers";
import { persistRoutingPolicy, type RoutingPolicy } from "../../routingPolicy";
import { useRelayState } from "../../state/RelayStateProvider";

export function RoutingPolicyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const planGroups = subscriptionPlanGroups(runtime?.accounts ?? [], t("common.unknown"));
  const defaultPlanOrder = planGroups.map((group) => group.id);
  const storedPlanOrder = runtime?.gateway.subscriptionPlanOrder ?? [];
  const initialPlanOrder = mergeSubscriptionPlanOrder(planGroups, storedPlanOrder);
  const initialStrategy = runtime?.gateway.routingStrategy ?? "adaptive";
  const [routingStrategy, setRoutingStrategy] = useState<RoutingStrategy>(initialStrategy);
  const defaultServiceTier = runtime?.gateway.defaultServiceTier ?? "standard";
  const [subscriptionPlanOrder, setSubscriptionPlanOrder] = useState(initialPlanOrder);
  const [draggedPlan, setDraggedPlan] = useState<string | null>(null);
  const [maxRetryCandidates, setMaxRetryCandidates] = useState(runtime?.gateway.maxRetryCandidates ?? 3);
  const [cooldownAfterFailures, setCooldownAfterFailures] = useState(runtime?.gateway.cooldownAfterFailures ?? 3);
  const [keepLastCandidateAvailable, setKeepLastCandidateAvailable] = useState(runtime?.gateway.keepLastCandidateAvailable ?? true);
  const hasCustomPlanOrder = subscriptionPlanOrder.length !== defaultPlanOrder.length || subscriptionPlanOrder.some((plan, index) => plan !== defaultPlanOrder[index]);
  const movePlan = (plan: string, target: string, after = false) => {
    if (plan === target) return;
    setSubscriptionPlanOrder((current) => {
      const next = current.filter((value) => value !== plan);
      const targetIndex = next.indexOf(target);
      if (targetIndex < 0) return current;
      next.splice(targetIndex + (after ? 1 : 0), 0, plan);
      return next;
    });
  };
  const movePlanBy = (plan: string, offset: number) => {
    const index = subscriptionPlanOrder.indexOf(plan);
    const target = subscriptionPlanOrder[index + offset];
    if (target) movePlan(plan, target, offset > 0);
  };
  const chooseStrategy = (value: string) => {
    setRoutingStrategy(value as RoutingStrategy);
  };
  const resetPlanOrder = () => {
    setSubscriptionPlanOrder(defaultPlanOrder);
  };
  const save = async () => {
    const savedPlanOrder = routingStrategy === "subscription_plan" ? subscriptionPlanOrder : [];
    const payload: RoutingPolicy = {
      maxRetryCandidates,
      cooldownAfterFailures,
      keepLastCandidateAvailable,
      routingStrategy,
      defaultServiceTier,
      subscriptionPlanOrder: savedPlanOrder,
    };
    const ok = await perform("routing-policy", () => persistRoutingPolicy(mode, payload), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("pool.routingSettingsTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "routing-policy"} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form pool-policy-form">
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.routingStrategy")}</strong><small>{t(`pool.routingStrategyHints.${routingStrategy}`)}</small></div>
        <OptionMenu className="field-option-menu pool-policy-control" label={t("pool.routingStrategy")} value={routingStrategy} onChange={chooseStrategy} options={[{ value: "adaptive", label: t("pool.routingStrategies.adaptive") }, { value: "quota_highest", label: t("pool.routingStrategies.quotaHighest") }, { value: "subscription_expiry", label: t("pool.routingStrategies.subscriptionExpiry") }, { value: "subscription_plan", label: t("pool.routingStrategies.subscriptionPlan") }]} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.maxRetryCandidates")}</strong><small>{t("pool.maxRetryCandidatesHint")}</small></div>
        <input className="pool-policy-control" aria-label={t("pool.maxRetryCandidates")} type="number" min={1} max={8} value={maxRetryCandidates} onChange={(event) => setMaxRetryCandidates(clampRoutingCount(event.target.value))} />
      </div>
      <div className="pool-policy-row">
        <div className="pool-policy-copy"><strong>{t("pool.cooldownAfterFailures")}</strong><small>{t("pool.cooldownAfterFailuresHint")}</small></div>
        <input className="pool-policy-control" aria-label={t("pool.cooldownAfterFailures")} type="number" min={1} max={8} value={cooldownAfterFailures} onChange={(event) => setCooldownAfterFailures(clampRoutingCount(event.target.value))} />
      </div>
      <label className="pool-policy-toggle toggle-row"><input type="checkbox" checked={keepLastCandidateAvailable} onChange={(event) => setKeepLastCandidateAvailable(event.target.checked)} /><span>{t("pool.keepLastCandidateAvailable")}</span></label>
      {routingStrategy === "subscription_plan" ? <div className="subscription-plan-policy">
        <div className="subscription-plan-policy-heading"><div><strong>{t("pool.subscriptionPlanOrder")}</strong><small>{t("pool.subscriptionPlanOrderHint")}</small></div>{hasCustomPlanOrder ? <IconButton label={t("pool.resetSubscriptionPlanOrder")} icon={<RotateCcw aria-hidden />} onClick={resetPlanOrder} /> : null}</div>
        {subscriptionPlanOrder.length ? <div className="subscription-plan-order" role="list" aria-label={t("pool.subscriptionPlanOrder")}>{subscriptionPlanOrder.map((plan, index) => {
          const group = planGroups.find((candidate) => candidate.id === plan);
          if (!group) return null;
          const drop = (event: DragEvent<HTMLDivElement>) => {
            event.preventDefault();
            if (draggedPlan) movePlan(draggedPlan, plan, subscriptionPlanOrder.indexOf(draggedPlan) < index);
            setDraggedPlan(null);
          };
          return <div key={plan} className="subscription-plan-order-row" role="listitem" draggable onDragStart={() => setDraggedPlan(plan)} onDragEnd={() => setDraggedPlan(null)} onDragOver={(event) => event.preventDefault()} onDrop={drop} data-subscription-plan={plan} data-dragging={draggedPlan === plan ? "true" : "false"}>
            <GripVertical aria-hidden />
            <span className="subscription-plan-rank">{index + 1}</span>
            <AccountPlanBadge planType={plan === "unknown" ? null : plan} unknown={t("common.unknown")} />
            <small>{t("pool.subscriptionPlanAccountCount", { count: group.count })}</small>
            <div className="inline-actions"><IconButton label={t("pool.moveSubscriptionPlanUp", { plan: group.label })} icon={<ArrowUp aria-hidden />} disabled={index === 0} onClick={() => movePlanBy(plan, -1)} /><IconButton label={t("pool.moveSubscriptionPlanDown", { plan: group.label })} icon={<ArrowDown aria-hidden />} disabled={index === subscriptionPlanOrder.length - 1} onClick={() => movePlanBy(plan, 1)} /></div>
          </div>;
        })}</div> : <p className="form-note">{t("pool.noSubscriptionPlanGroups")}</p>}
      </div> : null}
    </div>
  </Dialog>;
}
