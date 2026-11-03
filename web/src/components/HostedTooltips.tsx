import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { createPortal } from "react-dom";
import { positionTooltip } from "../../../src/src/features/relay/components/ui/tooltipPosition";
import type { TooltipPosition } from "../../../src/src/features/relay/components/ui/tooltipPosition";
import "./hosted-tooltips.css";

const selector = "[data-relay-tooltip]";
const hoverDelay = 550;
type Hint = { anchor: HTMLElement; label: string };

function findAnchor(target: EventTarget | null) {
  if (!(target instanceof Element)) return null;
  const anchor = target.closest<HTMLElement>(selector);
  if (!anchor?.closest(".hosted-app")) return null;
  const control = target.closest("button, input, select, textarea, a, summary, [tabindex]");
  return control && control !== anchor && anchor.contains(control) && anchor.tagName !== "LABEL" ? null : anchor;
}

function hintText(anchor: HTMLElement) {
  const label = anchor.dataset["relayTooltip"]?.trim() ?? "";
  const normalize = (value: string) => value.replace(/\s+/g, " ").trim();
  if (normalize(label) !== normalize(anchor.innerText)) return label;
  return [anchor, ...anchor.querySelectorAll<HTMLElement>("*")].some(node => node.clientWidth > 0
    && (node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1)) ? label : "";
}

// Browser-only hints use Relay's styling. A native popover keeps them above
// modal dialogs without bringing Desktop IPC or runtime into Hosted.
export function HostedTooltips() {
  const [hint, setHint] = useState<Hint | null>(null);
  const [position, setPosition] = useState<TooltipPosition | null>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const currentAnchor = useRef<HTMLElement | null>(null);
  const id = useId();

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const cancelTimer = () => { clearTimeout(timer); timer = undefined; };
    const hide = () => { cancelTimer(); currentAnchor.current = null; setHint(null); };
    const activate = (target: EventTarget | null, immediate: boolean) => {
      const anchor = findAnchor(target);
      if (!anchor || (immediate && !anchor.matches(":focus-visible"))) { hide(); return; }
      const label = hintText(anchor);
      if (!label) { hide(); return; }
      hide();
      currentAnchor.current = anchor;
      const show = () => {
        if (!anchor.isConnected || currentAnchor.current !== anchor) return;
        setPosition(null);
        setHint({ anchor, label });
      };
      if (immediate) show(); else timer = setTimeout(show, hoverDelay);
    };
    const over = (event: MouseEvent) => {
      if (findAnchor(event.target) !== findAnchor(event.relatedTarget)) activate(event.target, false);
    };
    const out = (event: MouseEvent) => {
      if (findAnchor(event.relatedTarget) === currentAnchor.current) return;
      if (currentAnchor.current === document.activeElement && currentAnchor.current?.matches(":focus-visible")) return;
      hide();
    };
    const focus = (event: FocusEvent) => activate(event.target, true);
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    document.addEventListener("mouseover", over, true);
    document.addEventListener("mouseout", out, true);
    document.addEventListener("focusin", focus, true);
    document.addEventListener("focusout", hide, true);
    document.addEventListener("pointerdown", hide, true);
    document.addEventListener("keydown", keydown, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    return () => {
      cancelTimer();
      document.removeEventListener("mouseover", over, true);
      document.removeEventListener("mouseout", out, true);
      document.removeEventListener("focusin", focus, true);
      document.removeEventListener("focusout", hide, true);
      document.removeEventListener("pointerdown", hide, true);
      document.removeEventListener("keydown", keydown, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
    };
  }, []);

  useLayoutEffect(() => {
    const tooltip = tooltipRef.current;
    if (!hint || !tooltip) return;
    tooltip.showPopover?.();
    const anchor = hint.anchor.getBoundingClientRect();
    const preferred = hint.anchor.dataset["relayTooltipPlacement"];
    const placement = preferred === "top" || preferred === "bottom" || preferred === "left" || preferred === "right"
      ? preferred : hint.anchor.closest(".relay-sidebar") ? "right" : "bottom";
    setPosition(positionTooltip(anchor, { width: tooltip.offsetWidth, height: tooltip.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight }, placement));
    const ids = new Set(hint.anchor.getAttribute("aria-describedby")?.split(/\s+/).filter(Boolean));
    ids.add(id);
    hint.anchor.setAttribute("aria-describedby", [...ids].join(" "));
    return () => {
      const remaining = hint.anchor.getAttribute("aria-describedby")?.split(/\s+/).filter(value => value && value !== id);
      if (remaining?.length) hint.anchor.setAttribute("aria-describedby", remaining.join(" "));
      else hint.anchor.removeAttribute("aria-describedby");
    };
  }, [hint, id]);

  useEffect(() => {
    if (!hint) return;
    const observer = new MutationObserver(() => {
      const label = hint.anchor.isConnected ? hintText(hint.anchor) : "";
      if (!label) setHint(null);
      else if (label !== hint.label) setHint({ anchor: hint.anchor, label });
    });
    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-relay-tooltip"] });
    return () => observer.disconnect();
  }, [hint]);

  const host = hint?.anchor.closest("dialog[open]") ?? hint?.anchor.closest(".hosted-app");
  return hint && host ? createPortal(<div ref={tooltipRef} id={id} role="tooltip" popover="manual"
    className="relay-tooltip hosted-tooltip" data-positioned={!!position} data-placement={position?.placement} data-instant="true"
    style={position ? { left: position.left, top: position.top, "--relay-tooltip-arrow-left": `${position.arrowLeft}px`, "--relay-tooltip-arrow-top": `${position.arrowTop}px` } as CSSProperties : undefined}>
    {hint.label}
  </div>, host) : null;
}
