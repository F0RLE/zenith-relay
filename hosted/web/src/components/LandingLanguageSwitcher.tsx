import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { locales } from "../i18n/catalog";
import { useLocale } from "../i18n/LocaleProvider";
import { LanguageFlag } from "./LanguageFlag";
import "./language-switcher.css";
import "./landing-language-switcher.css";

/** Opens left into free header space without moving its neighboring controls. */
export function LandingLanguageSwitcher({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { locale, setLocale, t } = useLocale();
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const restore = useRef<"trigger" | "previous" | "next" | null>(null);
  const search = useRef({ text: "", at: 0 });
  const selected = locales.findIndex(entry => entry.code === locale);
  const [active, setActive] = useState(selected);
  const [width, setWidth] = useState(40);
  const [edges, setEdges] = useState({ start: false, end: false });

  function syncEdges() {
    const element = list.current;
    if (!element) return;
    setEdges({ start: element.scrollLeft > 1, end: element.scrollLeft + element.clientWidth < element.scrollWidth - 1 });
  }

  function show(index = selected) {
    setActive(index);
    search.current = { text: "", at: 0 };
    restore.current = null;
    onOpenChange(true);
  }

  function close(focus: typeof restore.current = null) {
    restore.current = focus;
    onOpenChange(false);
  }

  useLayoutEffect(() => {
    if (open) list.current?.focus({ preventScroll: true });
    else if (restore.current) {
      const target = restore.current === "trigger" ? trigger.current
        : restore.current === "next" ? root.current?.nextElementSibling
        : root.current?.parentElement?.previousElementSibling;
      if (target instanceof HTMLElement) target.focus({ preventScroll: true });
      restore.current = null;
    }
  }, [open]);

  useLayoutEffect(() => {
    if (!open) return;
    const header = root.current?.closest(".landing-header-inner");
    const brand = header?.querySelector(".landing-brand");
    const measure = () => {
      const box = root.current?.getBoundingClientRect();
      if (!box || !brand) return;
      setWidth(Math.min(locales.length * box.width + (locales.length - 1) * 4, Math.max(box.width, box.right - brand.getBoundingClientRect().right - 12)));
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (header) observer.observe(header);
    if (brand) observer.observe(brand);
    window.addEventListener("resize", measure);
    return () => { observer.disconnect(); window.removeEventListener("resize", measure); };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    list.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
    syncEdges();
  }, [open, active, width]);

  useEffect(() => {
    if (!open) return;
    const element = list.current;
    const scroll = (event: WheelEvent) => {
      if (!element || event.ctrlKey || element.scrollWidth <= element.clientWidth) return;
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (!delta) return;
      event.preventDefault();
      element.scrollLeft += delta * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.clientWidth : 1);
    };
    const outside = (event: Event) => {
      if (!root.current?.contains(event.target as Node)) onOpenChange(false);
    };
    const blur = () => onOpenChange(false);
    element?.addEventListener("wheel", scroll, { passive: false });
    document.addEventListener("pointerdown", outside);
    document.addEventListener("focusin", outside);
    window.addEventListener("blur", blur);
    return () => {
      element?.removeEventListener("wheel", scroll);
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("focusin", outside);
      window.removeEventListener("blur", blur);
    };
  }, [open, onOpenChange]);

  function choose(index: number) {
    setLocale(locales[index]!.code);
    close("trigger");
  }

  return <div ref={root} className="language-switcher landing-language-switcher" data-open={open || undefined}>
    <button ref={trigger} type="button" role="combobox" className="key-button key-secondary language-trigger"
      aria-label={`${t("language.label")}: ${locales[selected]!.label}`} aria-haspopup="listbox" aria-expanded={open}
      aria-controls={open ? id : undefined} aria-hidden={open || undefined} tabIndex={open ? -1 : 0}
      onClick={() => show()} onKeyDown={event => {
        if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          show(event.key === "Home" ? 0 : event.key === "End" ? locales.length - 1 : selected);
        }
      }}><LanguageFlag locale={locale} /></button>
    <div ref={list} id={id} className="landing-language-choices" style={{ width }} role="listbox" aria-label={t("language.label")}
      data-scroll-start={edges.start || undefined} data-scroll-end={edges.end || undefined} onScroll={syncEdges}
      aria-orientation="horizontal" aria-activedescendant={open ? `${id}-${active}` : undefined}
      aria-hidden={!open || undefined} inert={!open} tabIndex={open ? 0 : -1}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); close("trigger"); return; }
        if (event.key === "Tab") { event.preventDefault(); close(event.shiftKey ? "previous" : "next"); return; }
        if (event.key === "Enter" || event.key === " ") { event.preventDefault(); choose(active); return; }
        if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          setActive(index => event.key === "Home" ? 0 : event.key === "End" ? locales.length - 1
            : (index + (["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : -1) + locales.length) % locales.length);
        } else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          const text = (Date.now() - search.current.at < 600 ? search.current.text : "") + event.key.toLocaleLowerCase();
          search.current = { text, at: Date.now() };
          const index = locales.findIndex(entry => entry.label.toLocaleLowerCase().startsWith(text) || entry.code.toLowerCase().startsWith(text));
          if (index >= 0) setActive(index);
        }
      }}>
      {locales.map((entry, index) => <button key={entry.code} id={`${id}-${index}`} type="button" role="option"
        className="landing-language-choice" lang={entry.code} aria-label={entry.label}
        aria-selected={entry.code === locale} data-active={active === index} tabIndex={-1}
        onPointerDown={event => { if (event.pointerType === "mouse") event.preventDefault(); }} onClick={() => choose(index)}>
        <LanguageFlag locale={entry.code} />
      </button>)}
    </div>
  </div>;
}
