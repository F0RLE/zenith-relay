import type { ReactNode } from "react";
import { useTooltip } from "../../components/Ui";

export function UsageMetric({ icon, label, value, detail, title, className }: {
  icon?: ReactNode;
  label: string;
  value: ReactNode;
  detail?: ReactNode;
  title?: string;
  className?: string;
}) {
  const tooltip = useTooltip<HTMLDivElement>(title ?? "");
  const hasTooltip = Boolean(title);
  return <>
    <div
      ref={hasTooltip ? tooltip.anchorRef : undefined}
      className={className}
      aria-describedby={hasTooltip ? tooltip.describedBy : undefined}
      onMouseEnter={hasTooltip ? tooltip.show : undefined}
      onMouseLeave={hasTooltip ? tooltip.hideAfterHover : undefined}
      onPointerDown={hasTooltip ? tooltip.pointerStart : undefined}
    >
      {icon}<div className="usage-metric-copy"><span>{label}</span><strong>{value}</strong>{detail ? <small>{detail}</small> : null}</div>
    </div>
    {hasTooltip ? tooltip.tooltip : null}
  </>;
}
