import { useEffect, useRef, useState } from "react";

export function useNavigationDrawer() {
  const [open, setOpen] = useState(false);
  const [mobile, setMobile] = useState(() => matchMedia("(max-width: 639px)").matches);
  const sidebar = useRef<HTMLElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const media = matchMedia("(max-width: 639px)");
    const resize = () => { setMobile(media.matches); setOpen(false); };
    media.addEventListener("change", resize);
    return () => media.removeEventListener("change", resize);
  }, []);
  useEffect(() => {
    if (!open || !mobile) return;
    const originalOverflow = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const focusable = () => [...(sidebar.current?.querySelectorAll<HTMLElement>('a[href], button:not(:disabled), summary, input, select, [tabindex="0"]') ?? [])].filter(element => element.getClientRects().length > 0);
    focusable()[0]?.focus({ preventScroll: true });
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented || document.querySelector("dialog[open]")) return;
      if (event.key === "Escape") { event.preventDefault(); setOpen(false); }
      if (event.key !== "Tab") return;
      const items = focusable();
      const first = items[0], last = items.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("keydown", key);
      document.body.style.overflow = originalOverflow;
      trigger.current?.focus({ preventScroll: true });
    };
  }, [open, mobile]);
  return { open, setOpen, mobile, sidebar, trigger };
}
