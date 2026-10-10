import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, MoreHorizontal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { mergeDescribedBy, useTooltip } from "./tooltip";

export function ActionMenu({ children, className = "", label }: { children: ReactNode; className?: string; label?: string }) {
  const { t } = useTranslation();
  const resolvedLabel = label ?? t("common.actions");
  const tooltip = useTooltip<HTMLElement>(resolvedLabel);
  const menuRef = useRef<HTMLDetailsElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const close = () => { menu.open = false; };
    const onPointerDown = (event: PointerEvent) => {
      // Stay attached from mount. A listener that waits for React to observe
      // the native open state misses a click that arrives in that gap.
      if (!menu.open || menu.contains(event.target as Node)) return;
      close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (!menu.open || event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      close();
      menu.querySelector("summary")?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = menuRef.current?.getBoundingClientRect();
      const panel = panelRef.current;
      if (!anchor || !panel || !menuRef.current?.open) return;
      panel.style.maxHeight = `${Math.max(0, innerHeight - 50)}px`;
      const { width, height } = panel.getBoundingClientRect();
      const below = anchor.bottom + 4;
      const top = below + height <= innerHeight - 8 ? below : anchor.top - height - 4;
      panel.style.top = `${Math.max(42, Math.min(top, innerHeight - height - 8)) - anchor.top}px`;
      panel.style.left = `${Math.max(8, Math.min(anchor.right - width, innerWidth - width - 8)) - anchor.left}px`;
      panel.style.right = "auto";
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, children]);
  return (
    <details ref={menuRef} className={`relay-action-menu ${className}`.trim()} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary
        ref={tooltip.anchorRef}
        aria-label={resolvedLabel}
        aria-describedby={tooltip.describedBy}
        aria-haspopup="menu"
        onMouseEnter={tooltip.show}
        onMouseLeave={tooltip.hideAfterHover}
        onFocus={tooltip.showAfterFocus}
        onBlur={tooltip.hide}
        onPointerDown={tooltip.pointerStart}
      >
        <MoreHorizontal aria-hidden />
      </summary>
      {tooltip.tooltip}
      <div ref={panelRef} className="relay-popover-panel" role="menu">{children}</div>
    </details>
  );
}

export function ActionMenuItem({ children, icon, danger = false, className = "", title, onClick, onMouseEnter, onMouseLeave, onFocus, onBlur, onPointerDown, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon: ReactNode; danger?: boolean }) {
  const tooltip = useTooltip<HTMLButtonElement>(title ?? "");
  const hasTooltip = Boolean(title);
  const classes = ["relay-popover-item", danger ? "danger" : "", className].filter(Boolean).join(" ");
  return <>
    <button
      ref={hasTooltip ? tooltip.anchorRef : undefined}
      type="button"
      role="menuitem"
      className={classes || undefined}
      {...props}
      aria-describedby={mergeDescribedBy(props["aria-describedby"], tooltip.describedBy)}
      onClick={(event) => { const menu = event.currentTarget.closest("details"); if (menu) menu.open = false; onClick?.(event); }}
      onMouseEnter={(event) => { if (hasTooltip) tooltip.show(); onMouseEnter?.(event); }}
      onMouseLeave={(event) => { if (hasTooltip) tooltip.hideAfterHover(); onMouseLeave?.(event); }}
      onFocus={(event) => { if (hasTooltip) tooltip.showAfterFocus(); onFocus?.(event); }}
      onBlur={(event) => { if (hasTooltip) tooltip.hide(); onBlur?.(event); }}
      onPointerDown={(event) => { if (hasTooltip) tooltip.pointerStart(); onPointerDown?.(event); }}
    >{icon}<span>{children}</span></button>
    {hasTooltip ? tooltip.tooltip : null}
  </>;
}

export function OptionMenu({ label, value, options, icon, onChange, className = "", disabled = false, align = "end", fitContent = false, listClassName = "", showSelectionIndicator = true }: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string; shortLabel?: string }>;
  icon?: ReactNode;
  onChange: (value: string) => void;
  className?: string;
  disabled?: boolean;
  align?: "start" | "center" | "end";
  fitContent?: boolean;
  listClassName?: string;
  showSelectionIndicator?: boolean;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number; width: number } | null>(null);
  const selected = options.find((option) => option.value === value) ?? options[0];

  const close = (restoreFocus = false) => {
    setOpen(false);
    setPosition(null);
    if (restoreFocus) triggerRef.current?.focus();
  };

  useLayoutEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current?.getBoundingClientRect();
    const list = listRef.current;
    if (!trigger || !list) return;
    const margin = 8;
    const gap = 6;
    const measuredWidth = () => {
      const labels = [...list.querySelectorAll<HTMLElement>("button > span")];
      const labelWidth = Math.max(0, ...labels.map((label) => {
        const range = document.createRange();
        range.selectNodeContents(label);
        return range.getBoundingClientRect().width;
      }));
      const firstOptionButton = list.querySelector<HTMLElement>("button");
      if (!firstOptionButton) return list.getBoundingClientRect().width;
      const pixels = (cssLength: string) => Number.parseFloat(cssLength) || 0;
      const itemStyle = window.getComputedStyle(firstOptionButton);
      const listStyle = window.getComputedStyle(list);
      const indicatorWidth = showSelectionIndicator ? 16 + pixels(itemStyle.columnGap) : 0;
      return Math.ceil(
        labelWidth
        + pixels(itemStyle.paddingLeft)
        + pixels(itemStyle.paddingRight)
        + indicatorWidth
        + pixels(listStyle.paddingLeft)
        + pixels(listStyle.paddingRight)
        + pixels(listStyle.borderLeftWidth)
        + pixels(listStyle.borderRightWidth),
      );
    };
    const preferredWidth = fitContent ? measuredWidth() : 220;
    const width = Math.min(fitContent ? preferredWidth : Math.max(trigger.width, preferredWidth), window.innerWidth - margin * 2);
    const preferredLeft = align === "start"
      ? trigger.left
      : align === "center"
        ? trigger.left + (trigger.width - width) / 2
        : trigger.right - width;
    const left = Math.max(margin, Math.min(preferredLeft, window.innerWidth - width - margin));
    const below = trigger.bottom + gap;
    const top = below + list.offsetHeight <= window.innerHeight - margin
      ? below
      : Math.max(margin, trigger.top - list.offsetHeight - gap);
    setPosition({ left, top, width });
  }, [align, fitContent, open, options.length, showSelectionIndicator]);

  useEffect(() => {
    if (!open) return;
    const selectedOption = listRef.current?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]');
    selectedOption?.focus({ preventScroll: true });
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !listRef.current?.contains(target)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close(true);
        return;
      }
      // A listbox is rendered in a portal. Closing it before a surrounding
      // Dialog handles Tab lets the dialog keep focus inside its own subtree
      // instead of leaving focus on a detached portal option.
      if (event.key === "Tab") close();
    };
    const dismiss = () => close();
    const dismissOnWheel = (event: WheelEvent) => {
      const target = event.target;
      if (target instanceof Node && listRef.current?.contains(target)) return;
      close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    // Capture Escape before a containing Dialog's document listener sees it.
    // The Dialog then observes defaultPrevented and stays open while the
    // listbox closes.
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("wheel", dismissOnWheel, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("wheel", dismissOnWheel, true);
    };
  }, [open]);

  const moveFocus = (event: React.KeyboardEvent<HTMLElement>, index: number) => {
    const direction = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : direction ? (index + direction + options.length) % options.length : -1;
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if (nextIndex < 0) return;
    event.preventDefault();
    listRef.current?.querySelectorAll<HTMLElement>('[role="option"]')[nextIndex]?.focus();
  };

  return <div className={`relay-option-menu ${className}`.trim()}>
    <button
      ref={triggerRef}
      type="button"
      className="relay-option-trigger"
      aria-label={`${label}: ${selected?.label ?? ""}`}
      aria-haspopup="listbox"
      aria-expanded={open}
      data-value={value}
      disabled={disabled}
      onClick={() => setOpen((previousOpen) => !previousOpen)}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.preventDefault();
          close(true);
          return;
        }
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        setOpen(true);
      }}
    >
      {icon}
      <span>{selected?.shortLabel ?? selected?.label}</span>
      <ChevronDown aria-hidden />
    </button>
    {open && typeof document !== "undefined" ? createPortal(
      <div
        ref={listRef}
        className={`relay-option-list relay-popover-panel ${listClassName}`.trim()}
        role="listbox"
        aria-label={label}
        data-positioned={Boolean(position)}
        data-selection-indicator={showSelectionIndicator}
        style={position ? { left: position.left, top: position.top, width: position.width } : undefined}
      >
        {options.map((option, index) => <button
          key={option.value}
          type="button"
          className="relay-popover-item"
          role="option"
          data-value={option.value}
          aria-selected={option.value === value}
          onClick={() => {
            onChange(option.value);
            close(true);
          }}
          onKeyDown={(event) => moveFocus(event, index)}
        >
          <span>{option.label}</span>
          {showSelectionIndicator && option.value === value ? <Check aria-hidden /> : null}
        </button>)}
      </div>,
      document.body,
    ) : null}
  </div>;
}

