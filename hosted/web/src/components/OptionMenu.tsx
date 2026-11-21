import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown } from "./Icons";

export function OptionMenu({ label, value, options, onChange, disabled = false, triggerClassName = "", listClassName = "", attached = false, iconOnly = false, excludeSelected = false, orientation = "vertical" }: {
  label: string;
  value: string;
  options: { value: string; label: string; shortLabel?: string; lang?: string; icon?: ReactNode }[];
  onChange: (value: string) => void;
  disabled?: boolean;
  triggerClassName?: string;
  listClassName?: string;
  attached?: boolean;
  iconOnly?: boolean;
  excludeSelected?: boolean;
  orientation?: "vertical" | "horizontal";
}) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const search = useRef({ text: "", at: 0 });
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const current = options.find(option => option.value === value) ?? options[0];
  const choices = excludeSelected && options.length > 1 ? options.filter(option => option.value !== value) : options;
  const selected = Math.max(0, choices.findIndex(option => option.value === value));

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      if (!trigger.current?.getClientRects().length) { setOpen(false); return; }
      const anchor = trigger.current?.getBoundingClientRect();
      const panel = list.current;
      if (!anchor || !panel) return;
      if (attached) {
        if (orientation === "horizontal") {
          const side = anchor.left >= panel.getBoundingClientRect().width + 8 ? "start" : "end";
          panel.dataset.side = side;
          trigger.current!.dataset.menuSide = side;
        }
        return;
      }
      panel.style.minWidth = `${Math.min(anchor.width, innerWidth - 16)}px`;
      const below = Math.max(0, innerHeight - anchor.bottom - 14);
      const above = Math.max(0, anchor.top - 14);
      const down = Math.min(panel.scrollHeight, 280) <= below || below >= above;
      panel.style.maxHeight = `${Math.min(280, down ? below : above)}px`;
      const { width, height } = panel.getBoundingClientRect();
      panel.style.left = `${Math.max(8, Math.min(anchor.left, innerWidth - width - 8))}px`;
      panel.style.top = `${down ? anchor.bottom + 6 : anchor.top - height - 6}px`;
      panel.style.visibility = "visible";
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, choices.length, attached, orientation]);

  useEffect(() => {
    if (!open) return;
    const dismiss = (event: Event) => {
      const target = event.target as Node;
      if (!trigger.current?.contains(target) && !list.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("focusin", dismiss);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("focusin", dismiss);
    };
  }, [open]);

  useEffect(() => {
    if (open) list.current?.children[active]?.scrollIntoView({ block: "nearest" });
  }, [open, active]);

  function choose(index: number) {
    const option = choices[index];
    if (!option) return;
    onChange(option.value);
    setOpen(false);
    trigger.current?.focus({ preventScroll: true });
  }

  const panel = open && <div ref={list} id={id} className={`option-list ${listClassName}`} role="listbox" aria-label={label} aria-orientation={orientation}>
    {choices.map((option, index) => <button type="button" role="option" tabIndex={-1} id={`${id}-${index}`}
      key={option.value} aria-selected={option.value === value} data-active={index === active}
      onPointerDown={event => event.preventDefault()} onPointerMove={() => setActive(index)} onClick={() => choose(index)}>
      <span className="option-value" lang={option.lang}>{option.icon}<span>{option.label}</span></span>
      {option.value === value ? <Check aria-hidden="true" /> : <span className="option-check" />}
    </button>)}
  </div>;

  return <div className="option-menu" data-attached={attached || undefined}>
    <button ref={trigger} type="button" role="combobox" className={`option-trigger ${triggerClassName}`} aria-label={iconOnly ? `${label}: ${current?.label}` : label}
      aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? id : undefined}
      aria-activedescendant={open ? `${id}-${active}` : undefined} disabled={disabled || choices.length === 0}
      onClick={() => { setActive(selected); search.current.text = ""; setOpen(!open); }}
      onKeyDown={event => {
        if (event.key === "Escape" && open) { event.preventDefault(); setOpen(false); return; }
        if (event.key === "Tab") { setOpen(false); return; }
        if (["Enter", " "].includes(event.key)) {
          event.preventDefault();
          if (open) choose(active);
          else { setActive(selected); setOpen(true); }
          return;
        }
        if (["ArrowDown", "ArrowUp", "Home", "End", ...(orientation === "horizontal" ? ["ArrowLeft", "ArrowRight"] : [])].includes(event.key)) {
          event.preventDefault();
          const next = event.key === "Home" ? 0 : event.key === "End" ? choices.length - 1
            : open ? (active + (["ArrowDown", "ArrowRight"].includes(event.key) ? 1 : -1) + choices.length) % choices.length : selected;
          setActive(next); setOpen(true); return;
        }
        if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          const text = (Date.now() - search.current.at < 600 ? search.current.text : "") + event.key.toLocaleLowerCase();
          search.current = { text, at: Date.now() };
          const index = choices.findIndex(option => option.label.toLocaleLowerCase().startsWith(text));
          if (index >= 0) { setActive(index); setOpen(true); }
        }
      }}><span className="option-value" lang={current?.lang}>{current?.icon}<span className={iconOnly ? "option-value-label-hidden" : undefined}>{current?.shortLabel ?? current?.label}</span></span><ChevronDown aria-hidden="true" /></button>
    {panel && (attached ? panel : createPortal(panel, trigger.current?.closest("dialog") ?? trigger.current?.closest(".relay-app") ?? document.body))}
  </div>;
}
