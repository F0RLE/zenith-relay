import { useEffect, useLayoutEffect, useState } from "react";
import { useTooltip } from "./Ui";

const hintSelector = "[data-relay-tooltip]";
const controlSelector = "button, input, select, textarea, a, summary, [tabindex]";
const normalizeHintText = (value: string) => value.replace(/\s+/g, " ").trim();

function hintTarget(target: EventTarget | null) {
  if (!(target instanceof Element)) return null;
  const anchor = target.closest<HTMLElement>(hintSelector);
  if (!anchor) return null;
  // A card/label hint must not compete with an inner button's own tooltip.
  const control = target.closest(controlSelector);
  if (control && anchor.contains(control) && control !== anchor && anchor.tagName !== "LABEL") return null;
  return anchor;
}

function hintLabel(anchor: HTMLElement) {
  const label = anchor.dataset["relayTooltip"]?.trim() ?? "";
  if (normalizeHintText(label) !== normalizeHintText(anchor.textContent ?? "")) return label;
  // Full identifiers and counters are useful only when the visible value is clipped.
  const clipped = [anchor, ...anchor.querySelectorAll<HTMLElement>("*")].some((node) =>
    node.clientWidth > 0 && (node.scrollWidth > node.clientWidth + 1 || node.scrollHeight > node.clientHeight + 1),
  );
  return clipped ? label : "";
}

/** One renderer for DOM hints: no wrappers that could alter tables, grids or drag handles. */
export function AppTooltips() {
  const [hint, setHint] = useState<{ label: string; target: HTMLElement } | null>(null);
  const { anchorRef, describedBy, show, showNow, hide, tooltip } = useTooltip<HTMLElement>(hint?.label ?? "");

  useEffect(() => {
    const activate = (event: Event, keyboard: boolean) => {
      const anchor = hintTarget(event.target);
      const text = anchor ? hintLabel(anchor) : "";
      if (!anchor || !text || (keyboard && !(event.target instanceof Element && event.target.matches(":focus-visible")))) {
        hide();
        return;
      }
      if (!keyboard && event instanceof MouseEvent && hintTarget(event.relatedTarget) === anchor) return;
      anchorRef.current = anchor;
      setHint({ label: text, target: keyboard && event.target instanceof HTMLElement ? event.target : anchor });
      if (keyboard) showNow(); else show();
    };
    const onOver = (event: MouseEvent) => activate(event, false);
    const onFocus = (event: FocusEvent) => activate(event, true);
    const onOut = (event: MouseEvent) => {
      const anchor = anchorRef.current;
      if (event.relatedTarget instanceof Node && anchor?.contains(event.relatedTarget)) return;
      if (anchor?.contains(document.activeElement) && document.activeElement?.matches(":focus-visible")) return;
      hide();
    };
    // Capture runs before React's component handlers: the more specific custom
    // control gets final ownership when a hint contains another tooltip trigger.
    document.addEventListener("mouseover", onOver, true);
    document.addEventListener("mouseout", onOut, true);
    document.addEventListener("focusin", onFocus, true);
    document.addEventListener("focusout", hide, true);
    return () => {
      document.removeEventListener("mouseover", onOver, true);
      document.removeEventListener("mouseout", onOut, true);
      document.removeEventListener("focusin", onFocus, true);
      document.removeEventListener("focusout", hide, true);
    };
  }, [anchorRef, hide, show, showNow]);

  useEffect(() => {
    if (describedBy) return;
    anchorRef.current = null;
    setHint(null);
  }, [anchorRef, describedBy]);

  useLayoutEffect(() => {
    const target = hint?.target;
    if (!target || !describedBy) return;
    const ids = new Set(target.getAttribute("aria-describedby")?.split(/\s+/).filter(Boolean));
    ids.add(describedBy);
    target.setAttribute("aria-describedby", [...ids].join(" "));
    return () => {
      const remaining = target.getAttribute("aria-describedby")?.split(/\s+/).filter((id) => id && id !== describedBy);
      if (remaining?.length) target.setAttribute("aria-describedby", remaining.join(" "));
      else target.removeAttribute("aria-describedby");
    };
  }, [describedBy, hint]);

  useEffect(() => {
    const anchor = anchorRef.current;
    if (!anchor || !describedBy) return;
    const observer = new MutationObserver(() => {
      const nextLabel = hintLabel(anchor);
      if (!nextLabel) hide();
      else setHint((current) => current && current.label !== nextLabel ? { ...current, label: nextLabel } : current);
    });
    observer.observe(anchor, { attributes: true, attributeFilter: ["data-relay-tooltip"], childList: true, characterData: true, subtree: true });
    return () => observer.disconnect();
  }, [anchorRef, describedBy, hide, hint?.target]);

  return tooltip;
}
