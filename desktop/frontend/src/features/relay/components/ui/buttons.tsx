import type { ReactNode } from "react";
import { Loader2 } from "lucide-react";
import { mergeDescribedBy, useTooltip } from "./tooltip";

export function Button({
  children,
  icon,
  variant = "secondary",
  busy,
  className,
  title,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onBlur,
  onPointerDown,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  icon?: ReactNode;
  variant?: "primary" | "secondary" | "ghost" | "danger";
  busy?: boolean;
}) {
  const tooltip = useTooltip<HTMLElement>(title ?? "");
  const hasTooltip = Boolean(title);
  const disabled = Boolean(busy || props.disabled);
  const button = <button
    ref={hasTooltip && !disabled ? (node) => { tooltip.anchorRef.current = node; } : undefined}
    type={props.type ?? "button"}
    className={`relay-button ${variant}${className ? ` ${className}` : ""}`}
    {...props}
    aria-describedby={mergeDescribedBy(props["aria-describedby"], tooltip.describedBy)}
    disabled={disabled}
    onMouseEnter={disabled ? undefined : (event) => { if (hasTooltip) tooltip.show(); onMouseEnter?.(event); }}
    onMouseLeave={disabled ? undefined : (event) => { if (hasTooltip) tooltip.hideAfterHover(); onMouseLeave?.(event); }}
    onFocus={disabled ? undefined : (event) => { if (hasTooltip) tooltip.showAfterFocus(); onFocus?.(event); }}
    onBlur={disabled ? undefined : (event) => { if (hasTooltip) tooltip.hide(); onBlur?.(event); }}
    onPointerDown={disabled ? undefined : (event) => { if (hasTooltip) tooltip.pointerStart(); onPointerDown?.(event); }}
  >
    {busy ? <Loader2 className="spin" aria-hidden /> : icon}<span>{children}</span>
  </button>;
  return <>
    {hasTooltip && disabled ? (
      <span
        ref={(node) => { tooltip.anchorRef.current = node; }}
        className="relay-disabled-tooltip-anchor"
        tabIndex={0}
        aria-describedby={tooltip.describedBy}
        onFocus={tooltip.showAfterFocus}
        onBlur={tooltip.hide}
        onMouseEnter={tooltip.show}
        onMouseLeave={tooltip.hideAfterHover}
      >
        {button}
      </span>
    ) : button}
    {hasTooltip ? tooltip.tooltip : null}
  </>;
}

export function IconButton({
  label,
  icon,
  busy = false,
  className = "",
  title,
  onMouseEnter,
  onMouseLeave,
  onFocus,
  onBlur,
  onPointerDown,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  label: string;
  icon: ReactNode;
  busy?: boolean;
}) {
  const tooltip = useTooltip<HTMLElement>(title ?? label);
  const disabled = Boolean(busy || props.disabled);
  const button = <button
    ref={disabled ? undefined : (node) => { tooltip.anchorRef.current = node; }}
    type={props.type ?? "button"}
    className={`relay-icon-button ${className}`.trim()}
    aria-label={label}
    {...props}
    disabled={disabled}
    aria-busy={busy || props["aria-busy"]}
    aria-describedby={mergeDescribedBy(props["aria-describedby"], tooltip.describedBy)}
    onMouseEnter={disabled ? undefined : (event) => { tooltip.show(); onMouseEnter?.(event); }}
    onMouseLeave={disabled ? undefined : (event) => { tooltip.hideAfterHover(); onMouseLeave?.(event); }}
    onFocus={disabled ? undefined : (event) => { tooltip.showAfterFocus(); onFocus?.(event); }}
    onBlur={disabled ? undefined : (event) => { tooltip.hide(); onBlur?.(event); }}
    onPointerDown={disabled ? undefined : (event) => { tooltip.pointerStart(); onPointerDown?.(event); }}
  >
    {busy ? <Loader2 className="spin" aria-hidden /> : icon}
  </button>;
  return <>
    {disabled ? (
      <span
        ref={(node) => { tooltip.anchorRef.current = node; }}
        className="relay-disabled-tooltip-anchor"
        tabIndex={0}
        aria-describedby={tooltip.describedBy}
        onFocus={tooltip.showAfterFocus}
        onBlur={tooltip.hide}
        onMouseEnter={tooltip.show}
        onMouseLeave={tooltip.hideAfterHover}
      >
        {button}
      </span>
    ) : button}
    {tooltip.tooltip}
  </>;
}

