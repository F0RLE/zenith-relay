import { useRef } from "react";
import { Cpu, Gauge, Rocket, Zap } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { PoolActivityPresentation } from "./poolActivity";
import type { PoolRoutingPolicy } from "./poolRotation";
import "./owner-pool-runtime.css";

export function OwnerPoolRuntime({ activity, mode }: { activity: PoolActivityPresentation; mode: PoolRoutingPolicy["mode"] }) {
  const { t } = useLocale();
  const current = activity.current.map(member => member.name).join(" · ");
  const summary = current ? t("pool.activity.current", { name: current })
    : activity.last ? t("pool.activity.last", { name: activity.last.name })
    : t(activity.observed ? "pool.activity.idle" : "pool.activity.unavailable");
  const models = activity.models.map(model => model.requests > 1 ? t("pool.activity.modelCount", { model: model.id, count: model.requests }) : model.id).join(" · ");
  const requests = activity.total ? t(models ? "pool.activity.requests" : "pool.activity.requestsUnknown", { count: activity.total, models }) : "";
  return <div className="owner-pool-runtime" aria-live="polite" aria-atomic="true">
    <h2>{t(mode === "automatic" ? "pool.activity.distribution" : mode === "failover" ? "pool.rotation.modes.failover" : "pool.activity.order")}</h2>
    <div className="owner-pool-route"><strong data-working={activity.total > 0 || undefined} data-relay-tooltip={summary}>{summary}</strong></div>
    <span className="owner-pool-active-models" data-active-request-count={activity.total} aria-hidden={!requests} data-relay-tooltip={requests || undefined}>{requests && <><Cpu aria-hidden="true" /><span>{requests}</span></>}</span>
  </div>;
}

const speeds = [
  { value: "standard", icon: Gauge }, { value: "fast", icon: Zap }, { value: "ultrafast", icon: Rocket },
] as const;
export type OwnerPoolSpeed = typeof speeds[number]["value"];

export function OwnerPoolSpeedControl({ value, onChange }: { value: OwnerPoolSpeed; onChange: (value: OwnerPoolSpeed) => void }) {
  const { t } = useLocale();
  const group = useRef<HTMLDivElement>(null);
  const select = (index: number) => { onChange(speeds[index]!.value); group.current?.querySelectorAll<HTMLButtonElement>("button")[index]?.focus(); };
  return <div ref={group} className="owner-pool-speed" role="radiogroup" aria-label={t("pool.speed.title")} data-speed={value} onKeyDown={event => {
    const offset = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
    if (offset === undefined && event.key !== "Home" && event.key !== "End") return;
    event.preventDefault();
    const index = speeds.findIndex(mode => mode.value === value);
    select(event.key === "Home" ? 0 : event.key === "End" ? speeds.length - 1 : Math.max(0, Math.min(speeds.length - 1, index + (offset ?? 0))));
  }}>
    {speeds.map(({ value: tier, icon: Icon }) => <button key={tier} type="button" role="radio" aria-checked={value === tier} tabIndex={value === tier ? 0 : -1} data-relay-tooltip={t(`pool.speed.${tier}`)} onClick={() => onChange(tier)}><Icon aria-hidden="true" /><span className={value === tier ? undefined : "sr-only"}>{t(`pool.speed.${tier}`)}</span></button>)}
  </div>;
}
