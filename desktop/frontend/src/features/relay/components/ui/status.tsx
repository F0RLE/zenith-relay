import type { ReactNode } from "react";
import { CheckCircle2, CircleAlert, CircleHelp, CircleOff } from "lucide-react";
import { useTooltip } from "./tooltip";

export function StatusIcon({ status, label, className = "", children, showTooltip = true }: { status: "ready" | "warning" | "error" | "info" | "disabled"; label: string; className?: string; children?: ReactNode; showTooltip?: boolean }) {
  const tooltip = useTooltip<HTMLSpanElement>(label);
  return <>
    <span
      ref={tooltip.anchorRef}
      className={`relay-status-icon ${className}`.trim()}
      data-status={status}
      role="img"
      tabIndex={0}
      aria-label={label}
      aria-describedby={showTooltip ? tooltip.describedBy : undefined}
      onMouseEnter={showTooltip ? tooltip.show : undefined}
      onMouseLeave={showTooltip ? tooltip.hideAfterHover : undefined}
      onFocus={showTooltip ? tooltip.showAfterFocus : undefined}
      onBlur={showTooltip ? tooltip.hide : undefined}
      onPointerDown={showTooltip ? tooltip.pointerStart : undefined}
    >
      {children ?? <StatusBadge status={status} label="" />}
    </span>
    {showTooltip ? tooltip.tooltip : null}
  </>;
}


export function StatusBadge({ status, label }: { status: "ready" | "warning" | "error" | "info" | "disabled"; label: string }) {
  const Icon = status === "ready" ? CheckCircle2 : status === "disabled" ? CircleOff : status === "info" ? CircleHelp : CircleAlert;
  return <span className={`relay-status ${status}`}><Icon aria-hidden />{label}</span>;
}

