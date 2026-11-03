import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { locales } from "../i18n/catalog";
import { useLocale } from "../i18n/LocaleProvider";
import { ChevronLeft, ChevronRight } from "./Icons";
import { LanguageFlag } from "./LanguageFlag";
import "./language-switcher.css";
import "./header-language-switcher.css";

const wrap = (index: number) => ((index % locales.length) + locales.length) % locales.length;

/** A draft language replaces the two neighboring utilities until explicitly chosen. */
export function HeaderLanguageSwitcher({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { locale, setLocale, t } = useLocale();
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const rail = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const restore = useRef<"trigger" | "previous" | "next" | null>(null);
  const wheel = useRef(0);
  const search = useRef({ text: "", at: 0 });
  const selected = locales.findIndex(entry => entry.code === locale);
  // Unwrapped positions retain each flag's DOM identity as it rolls through the rail.
  const [cursor, setCursor] = useState(selected);
  const [session, setSession] = useState(0);
  const current = locales[selected]!;
  const slots = [-2, -1, 0, 1, 2].map(offset => cursor + offset);

  function show(index = selected) {
    setCursor(index);
    setSession(value => value + 1);
    wheel.current = 0;
    search.current = { text: "", at: 0 };
    restore.current = null;
    onOpenChange(true);
  }

  function close(focus: typeof restore.current = null) {
    restore.current = focus;
    onOpenChange(false);
  }

  function choose(index: number) {
    setCursor(index);
    setLocale(locales[wrap(index)]!.code);
    close("trigger");
  }

  function preview(index: number) {
    setCursor(current => {
      const distance = wrap(index - wrap(current));
      return current + (distance > locales.length / 2 ? distance - locales.length : distance);
    });
  }

  useLayoutEffect(() => {
    if (open) list.current?.focus({ preventScroll: true });
    else if (restore.current) {
      const target = restore.current === "trigger" ? trigger.current
        : restore.current === "next" ? root.current?.nextElementSibling : root.current?.previousElementSibling;
      if (target instanceof HTMLElement) target.focus({ preventScroll: true });
      restore.current = null;
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const element = rail.current;
    const scroll = (event: WheelEvent) => {
      if (event.ctrlKey) return; // Preserve browser zoom gestures.
      const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
      if (!delta) return;
      event.preventDefault();
      const normalized = delta * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 120 : 1);
      if (Math.sign(normalized) !== Math.sign(wheel.current)) wheel.current = 0;
      wheel.current += normalized;
      if (Math.abs(wheel.current) >= 40) {
        const direction = Math.sign(wheel.current);
        setCursor(index => index + direction);
        wheel.current = 0;
      }
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

  return <div ref={root} className="language-switcher language-roller" data-open={open || undefined}>
    <button ref={trigger} type="button" role="combobox" className="language-roller-trigger" aria-label={`${t("language.label")}: ${current.label}`}
      aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? id : undefined} aria-hidden={open || undefined} tabIndex={open ? -1 : 0}
      onClick={() => show()} onKeyDown={event => {
        if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
          event.preventDefault(); show(event.key === "Home" ? 0 : event.key === "End" ? locales.length - 1 : selected);
        }
      }}><LanguageFlag locale={locale} /></button>
    <div ref={rail} className="language-roller-rail" aria-hidden={!open || undefined} inert={!open}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close("trigger"); return; }
        if (event.key === "Tab") { event.preventDefault(); close(event.shiftKey ? "previous" : "next"); return; }
        if (["Enter", " "].includes(event.key)) {
          if (event.target instanceof HTMLElement && event.target.closest(".language-roller-step")) return;
          event.preventDefault(); choose(cursor); return;
        }
        if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
          event.preventDefault();
          if (event.key === "Home" || event.key === "End") preview(event.key === "Home" ? 0 : locales.length - 1);
          else setCursor(index => index + (["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : -1));
          return;
        }
        if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          const text = (Date.now() - search.current.at < 600 ? search.current.text : "") + event.key.toLocaleLowerCase();
          search.current = { text, at: Date.now() };
          const index = locales.findIndex(entry => entry.label.toLocaleLowerCase().startsWith(text) || entry.code.toLowerCase().startsWith(text));
          if (index >= 0) preview(index);
        }
      }}>
      <button type="button" className="language-roller-step" data-direction="previous" tabIndex={-1}
        aria-label={`${t("language.previous")}: ${locales[wrap(cursor - 1)]!.label}`}
        onPointerDown={event => event.preventDefault()} onClick={() => { setCursor(index => index - 1); list.current?.focus({ preventScroll: true }); }}><ChevronLeft /></button>
      <div ref={list} id={id} className="language-roller-list" role="listbox" tabIndex={open ? 0 : -1} aria-label={t("language.label")} aria-hidden={!open || undefined} inert={!open}
        aria-orientation="horizontal" aria-activedescendant={open ? `${id}-${session}-${cursor}` : undefined}>
      {slots.map(index => {
        const entry = locales[wrap(index)]!;
        const offset = index - cursor;
        return <button type="button" role="option" key={`${session}-${index}`} id={`${id}-${session}-${index}`} lang={entry.code} aria-label={entry.label}
          aria-setsize={locales.length} aria-posinset={wrap(index) + 1}
          aria-selected={index === cursor} aria-hidden={offset !== 0 || undefined} data-active={index === cursor} data-outside={offset !== 0 || undefined} tabIndex={-1}
          style={{ "--language-slot": offset } as CSSProperties}
          onPointerDown={event => event.preventDefault()} onClick={() => choose(index)}><LanguageFlag locale={entry.code} /></button>;
      })}
      </div>
      <button type="button" className="language-roller-step" data-direction="next" tabIndex={-1}
        aria-label={`${t("language.next")}: ${locales[wrap(cursor + 1)]!.label}`}
        onPointerDown={event => event.preventDefault()} onClick={() => { setCursor(index => index + 1); list.current?.focus({ preventScroll: true }); }}><ChevronRight /></button>
    </div>
  </div>;
}
