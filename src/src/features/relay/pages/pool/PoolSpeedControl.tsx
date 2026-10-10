import { useEffect, useRef } from "react";
import { Gauge, Rocket, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { DefaultServiceTier } from "../../api/types";

const SPEED_MODES = [
  { tier: "standard", icon: Gauge },
  { tier: "fast", icon: Zap },
  { tier: "ultrafast", icon: Rocket },
] as const;

export function PoolSpeedControl({
  value,
  disabled,
  saving,
  onChange,
  className = "",
  modelId,
  tiers,
  iconsOnly = false,
}: {
  value: DefaultServiceTier;
  disabled: boolean;
  saving: boolean;
  onChange: (value: DefaultServiceTier) => void;
  className?: string;
  modelId?: string;
  tiers?: readonly DefaultServiceTier[];
  iconsOnly?: boolean;
}) {
  const { t } = useTranslation();
  const groupRef = useRef<HTMLDivElement>(null);
  const modes = SPEED_MODES.filter(({ tier }) => !tiers || tiers.includes(tier));
  const available = modes.length ? modes : SPEED_MODES;
  const selected = available.some(({ tier }) => tier === value) ? value : available[0]!.tier;
  const selectTier = (tier: DefaultServiceTier) => {
    if (tier !== selected && !disabled) onChange(tier);
  };
  useEffect(() => {
    const group = groupRef.current;
    if (!group?.contains(document.activeElement)) return;
    group.querySelector<HTMLButtonElement>("[aria-checked='true']")?.focus();
  }, [selected]);
  const move = (offset: number) => {
    const selectedIndex = available.findIndex(({ tier }) => tier === selected);
    const nextTier = available[Math.min(available.length - 1, Math.max(0, selectedIndex + offset))];
    if (nextTier) selectTier(nextTier.tier);
  };

  return <div
    ref={groupRef}
    className={`pool-speed-control${iconsOnly ? " icons-only" : ""}${className ? ` ${className}` : ""}`}
    role="radiogroup"
    aria-label={t("pool.serviceTier")}
    data-speed-tier={selected}
    {...(modelId ? { "data-model-speed-select": modelId } : { "data-pool-speed-select": "true" })}
    aria-busy={saving}
    onKeyDown={(event) => {
      if (event.key === "ArrowRight" || event.key === "ArrowDown") { event.preventDefault(); move(1); }
      else if (event.key === "ArrowLeft" || event.key === "ArrowUp") { event.preventDefault(); move(-1); }
      else if (event.key === "Home") { event.preventDefault(); selectTier(available[0]!.tier); }
      else if (event.key === "End") { event.preventDefault(); selectTier(available[available.length - 1]!.tier); }
    }}
  >
    {available.map(({ tier, icon: Icon }) => {
      const active = tier === selected;
      const label = t(`pool.serviceTiers.${tier}`);
      return <button
        key={tier}
        type="button"
        role="radio"
        className={active ? "active" : ""}
        aria-checked={active}
        tabIndex={active ? 0 : -1}
        disabled={disabled}
        data-relay-tooltip={label}
        onClick={() => selectTier(tier)}
      >
        <Icon aria-hidden />
        <span className={active && !iconsOnly ? undefined : "sr-only"}>{label}</span>
      </button>;
    })}
  </div>;
}
