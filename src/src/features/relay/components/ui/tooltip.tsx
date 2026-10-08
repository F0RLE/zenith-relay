import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { createPortal } from "react-dom";

export function mergeDescribedBy(...values: Array<string | undefined>) {
  const describedByIds = new Set(values.flatMap((describedByValue) => describedByValue?.split(/\s+/).filter(Boolean) ?? []));
  return describedByIds.size ? [...describedByIds].join(" ") : undefined;
}

// Both component-owned and delegated hints share one visible tooltip.
let dismissActiveTooltip: (() => void) | undefined;

export function useTooltip<T extends HTMLElement>(label: string) {
  const anchorRef = useRef<T>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const tooltipId = useId();
  const [visible, setVisible] = useState(false);
  const [instant, setInstant] = useState(false);
  const [activation, setActivation] = useState(0);
  const [position, setPosition] = useState<{ left: number; top: number; placement: "top" | "bottom"; arrowLeft: number } | null>(null);

  const hide = useCallback(() => {
    setVisible(false);
    if (dismissActiveTooltip === hide) dismissActiveTooltip = undefined;
  }, []);
  const activate = useCallback((immediate: boolean) => {
    if (dismissActiveTooltip !== hide) dismissActiveTooltip?.();
    dismissActiveTooltip = hide;
    setInstant(immediate);
    setPosition(null);
    setActivation((activation) => activation + 1);
    setVisible(true);
  }, [hide]);
  const showNow = useCallback(() => activate(true), [activate]);
  const show = useCallback(() => activate(false), [activate]);
  const pointerStart = () => {
    hide();
  };

  useLayoutEffect(() => {
    if (!visible || !label) return;
    const anchor = anchorRef.current?.getBoundingClientRect();
    const tooltip = tooltipRef.current;
    if (!anchor || !tooltip) return;
    const margin = 9;
    // The 7px pointer and a small breathing space keep the arrow connected to
    // its trigger without making the tooltip feel attached to the control.
    const gap = 10;
    let placement: "top" | "bottom" = "bottom";
    let top = anchor.bottom + gap;
    if (top + tooltip.offsetHeight > window.innerHeight - margin && anchor.top - tooltip.offsetHeight - gap >= margin) {
      placement = "top";
      top = anchor.top - tooltip.offsetHeight - gap;
    }
    const anchorCenter = anchor.left + anchor.width / 2;
    const centered = anchorCenter - tooltip.offsetWidth / 2;
    const left = Math.max(margin, Math.min(centered, window.innerWidth - tooltip.offsetWidth - margin));
    // Keep the arrow on the trigger even when the tooltip itself must be
    // clamped inside the viewport.
    const arrowInset = 8;
    const arrowLeft = Math.max(
      arrowInset,
      Math.min(anchorCenter - left, tooltip.offsetWidth - arrowInset),
    );
    top = Math.max(margin, Math.min(top, window.innerHeight - tooltip.offsetHeight - margin));
    setPosition({ left, top, placement, arrowLeft });
  }, [label, visible, activation]);

  useEffect(() => () => {
    if (dismissActiveTooltip === hide) dismissActiveTooltip = undefined;
  }, [hide]);

  useEffect(() => {
    if (!visible) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || dismissActiveTooltip !== hide) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      hide();
    };
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    window.addEventListener("scroll", hide, true);
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("pointerdown", hide, true);
    const observer = new MutationObserver(() => {
      if (!anchorRef.current?.isConnected) hide();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
      window.removeEventListener("scroll", hide, true);
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("pointerdown", hide, true);
    };
  }, [hide, visible]);

  const tooltip = visible && label && typeof document !== "undefined" ? createPortal(
    <div
      ref={tooltipRef}
      id={tooltipId}
      className="relay-tooltip"
      role="tooltip"
      data-placement={position?.placement}
      data-positioned={Boolean(position)}
      data-instant={instant}
      style={position ? {
        left: position.left,
        top: position.top,
        "--relay-tooltip-arrow-left": `${position.arrowLeft}px`,
      } as CSSProperties : undefined}
    >
      {label}
    </div>,
    document.body,
  ) : null;

  return {
    anchorRef,
    describedBy: visible && label ? tooltipId : undefined,
    hide,
    hideAfterHover: () => { if (!anchorRef.current?.matches(":focus-visible")) hide(); },
    show,
    showNow,
    showAfterFocus: () => { if (anchorRef.current?.matches(":focus-visible")) showNow(); },
    pointerStart,
    tooltip,
  };
}

