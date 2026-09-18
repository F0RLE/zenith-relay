import { useRef, useState, type CSSProperties } from "react";
import { Gauge, Loader2, Rocket, Zap } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { DefaultServiceTier } from "../../api/types";

const SPEED_MODES = [
  { tier: "standard", icon: Gauge },
  { tier: "fast", icon: Zap },
  { tier: "ultrafast", icon: Rocket },
] as const;

export function PoolSpeedControl({ value, disabled, saving, onChange }: {
  value: DefaultServiceTier;
  disabled: boolean;
  saving: boolean;
  onChange: (value: DefaultServiceTier) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<number | null>(null);
  const dragPointer = useRef<number | null>(null);
  const position = draft ?? SPEED_MODES.findIndex(({ tier }) => tier === value);
  const { tier, icon: Icon } = SPEED_MODES[position] ?? SPEED_MODES[0];
  const selectPosition = (next: number) => {
    const option = SPEED_MODES[next];
    if (option) onChange(option.tier);
  };
  const cancelDrag = () => {
    dragPointer.current = null;
    setDraft(null);
  };

  return <div
    className="pool-speed-control"
    data-speed-tier={tier}
    data-pool-speed-select="true"
    data-relay-tooltip={t("pool.serviceTier")}
    aria-busy={saving}
    style={{ "--pool-speed-position": position } as CSSProperties}
  >
    <span className="pool-speed-current" aria-hidden>
      <span className="pool-speed-label" key={tier}>
        {saving ? <Loader2 className="spin" /> : <Icon />}
        <span>{t(`pool.serviceTiers.${tier}`)}</span>
      </span>
    </span>
    <div className="pool-speed-switch">
      <span className="pool-speed-rail" aria-hidden />
      <span className="pool-speed-stops" aria-hidden><i /><i /><i /></span>
      <span className="pool-speed-selection" aria-hidden />
      <input
        className="pool-speed-slider"
        type="range"
        min={0}
        max={SPEED_MODES.length - 1}
        step={1}
        value={position}
        aria-label={t("pool.serviceTier")}
        aria-valuetext={t(`pool.serviceTiers.${tier}`)}
        disabled={disabled || saving}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          dragPointer.current = event.pointerId;
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onChange={(event) => {
          const next = event.currentTarget.valueAsNumber;
          // Dragging previews a tier; only release persists it to the runtime.
          if (dragPointer.current !== null) setDraft(next);
          else selectPosition(next);
        }}
        onPointerUp={(event) => {
          if (dragPointer.current !== event.pointerId) return;
          const next = event.currentTarget.valueAsNumber;
          cancelDrag();
          selectPosition(next);
        }}
        onPointerCancel={cancelDrag}
        onLostPointerCapture={cancelDrag}
        onBlur={cancelDrag}
      />
    </div>
  </div>;
}
