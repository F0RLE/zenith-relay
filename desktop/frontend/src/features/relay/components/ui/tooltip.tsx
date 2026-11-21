import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { createPortal } from "react-dom";
import { hasTooltipKeyboardFocus, isTooltipFocusSuppressed } from "./focus";
import { positionTooltip } from "./tooltipPosition";
import type { TooltipPosition } from "./tooltipPosition";

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
  const [position, setPosition] = useState<TooltipPosition | null>(null);

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
    setPosition(positionTooltip(anchor, { width: tooltip.offsetWidth, height: tooltip.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight }, anchorRef.current?.closest(".relay-sidebar") ? "right" : "bottom"));
  }, [label, visible, activation]);

  useEffect(() => () => {
    if (dismissActiveTooltip === hide) dismissActiveTooltip = undefined;
  }, [hide]);

  useEffect(() => {
    if (!visible) return;
    const onScroll = (event: Event) => {
      const anchor = anchorRef.current;
      if (!anchor || !hasTooltipKeyboardFocus(anchor)) {
        hide();
        return;
      }
      const rect = anchor.getBoundingClientRect();
      const container = event.target instanceof Element && event.target.contains(anchor)
        ? event.target.getBoundingClientRect() : null;
      if (rect.bottom <= Math.max(0, container?.top ?? 0)
        || rect.top >= Math.min(window.innerHeight, container?.bottom ?? window.innerHeight)
        || rect.right <= Math.max(0, container?.left ?? 0)
        || rect.left >= Math.min(window.innerWidth, container?.right ?? window.innerWidth)) {
        hide();
        return;
      }
      // Keyboard navigation scrolls its focused trigger into view after focusin.
      setActivation((activation) => activation + 1);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || dismissActiveTooltip !== hide) return;
      // A hint must not swallow an editor's Escape-to-cancel shortcut.
      if (event.target instanceof HTMLElement && event.target.closest("input, textarea, [contenteditable='true']")) {
        hide();
        return;
      }
      // A passive hint must not make closing a dialog require a second Escape.
      // Dialogs and inner editors/menus retain their own dismissal ordering.
      if (anchorRef.current?.closest("[data-relay-dialog]")
        || event.target instanceof Element && event.target.closest("[data-relay-dialog]")) {
        hide();
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      hide();
    };
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    window.addEventListener("scroll", onScroll, true);
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
      window.removeEventListener("scroll", onScroll, true);
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
        "--relay-tooltip-arrow-top": `${position.arrowTop}px`,
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
    showAfterFocus: () => { if (!isTooltipFocusSuppressed() && anchorRef.current?.matches(":focus-visible")) showNow(); },
    pointerStart,
    tooltip,
  };
}

