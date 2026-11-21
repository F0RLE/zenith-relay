import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Calendar, ChevronDown, ChevronLeft, ChevronRight, ChevronUp } from "./Icons";
import { calendarDate, calendarDays, calendarWeekStart, displayCalendarDate, moveCalendarDays, moveCalendarMonths, parseCalendarInput, todayCalendarISO } from "./calendar-date";
import { useLocale } from "../i18n/LocaleProvider";
import "./date-picker.css";

export function DatePicker({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const { t, locale } = useLocale();
  const previousLocale = useRef(locale);
  const monthLabel = new Intl.DateTimeFormat(locale, { month: "long", year: "numeric", timeZone: "UTC" });
  const dayLabel = new Intl.DateTimeFormat(locale, { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
  const weekdayLabel = new Intl.DateTimeFormat(locale, { weekday: "short", timeZone: "UTC" });
  const weekdays = Array.from({ length: 7 }, (_, index) => weekdayLabel.format(calendarDate(moveCalendarDays("2024-01-07", index + calendarWeekStart(locale)))));
  const months = Array.from({ length: 12 }, (_, month) => new Intl.DateTimeFormat(locale, { month: "long", timeZone: "UTC" }).format(calendarDate(`2024-${String(month + 1).padStart(2, "0")}-01`)));
  const invalidDateMessage = t("calendar.invalidDate", { format: t("calendar.inputFormat").toLocaleUpperCase(locale) });
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLDivElement>(null);
  const emitted = useRef(value);
  const focusDay = useRef(false);
  const [text, setText] = useState(displayCalendarDate(value, locale));
  const [invalid, setInvalid] = useState(false);
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<"days" | "months" | "years">("days");
  const [active, setActive] = useState(value || todayCalendarISO());
  const today = todayCalendarISO();
  const days = calendarDays(active, locale);
  const activeYear = calendarDate(active).getUTCFullYear();
  const activeMonth = calendarDate(active).getUTCMonth();
  const yearStart = Math.min(9988, Math.floor((activeYear - 1) / 12) * 12 + 1);
  const periods = view === "months" ? months.map((name, index) => ({ value: index, label: name }))
    : Array.from({ length: 12 }, (_, index) => ({ value: yearStart + index, label: String(yearStart + index) }));
  const activePeriod = view === "months" ? activeMonth : activeYear;
  const heading = view === "days" ? monthLabel.format(calendarDate(active)) : view === "months" ? String(activeYear) : `${yearStart}–${yearStart + 11}`;
  const step = view === "days" ? 1 : view === "months" ? 12 : 144;

  useEffect(() => {
    if (value !== emitted.current || locale !== previousLocale.current && parseCalendarInput(text, previousLocale.current) !== null) {
      setText(displayCalendarDate(value, locale));
      setInvalid(false);
      input.current?.setCustomValidity("");
    }
    emitted.current = value;
    previousLocale.current = locale;
  }, [value, locale]);
  useEffect(() => { if (invalid) input.current?.setCustomValidity(invalidDateMessage); }, [invalid, invalidDateMessage]);

  function close(restoreFocus = false) {
    setOpen(false);
    if (restoreFocus) trigger.current?.focus({ preventScroll: true });
  }

  function show() {
    setActive(parseCalendarInput(text, locale) || value || today);
    setView("days");
    focusDay.current = true;
    setOpen(true);
  }

  function choose(iso: string) {
    setText(displayCalendarDate(iso, locale));
    setInvalid(false);
    input.current?.setCustomValidity("");
    emitted.current = iso;
    onChange(iso);
    close(true);
  }

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = field.current?.getBoundingClientRect(), popup = panel.current;
      if (!anchor || !popup || !field.current?.getClientRects().length) { setOpen(false); return; }
      const viewport = window.visualViewport;
      const top = viewport?.offsetTop ?? 0, left = viewport?.offsetLeft ?? 0;
      const width = viewport?.width ?? innerWidth, height = viewport?.height ?? innerHeight;
      if (anchor.bottom <= top || anchor.top >= top + height) { setOpen(false); return; }
      const form = input.current?.form;
      const owner = form?.getBoundingClientRect();
      const ownerStyle = form && getComputedStyle(form);
      const contentLeft = owner ? owner.left + parseFloat(ownerStyle!.paddingLeft) : left + 8;
      const contentRight = owner ? owner.right - parseFloat(ownerStyle!.paddingRight) : left + width - 8;
      // Align with the form's content edge; on narrow phones use its outer frame,
      // rather than squeezing seven day targets into the field's padded width.
      const useContent = contentRight - contentLeft >= 280;
      const minLeft = Math.max(left + 8, useContent ? contentLeft : owner?.left ?? left + 8);
      const maxRight = Math.min(left + width - 8, useContent ? contentRight : owner?.right ?? left + width - 8);
      popup.style.width = `${Math.max(0, Math.min(324, maxRight - minLeft))}px`;
      const below = Math.max(0, Math.min(height - 16, top + height - anchor.bottom - 14));
      const above = Math.max(0, Math.min(height - 16, anchor.top - top - 14));
      const down = popup.scrollHeight <= below || below >= above;
      popup.style.maxHeight = `${Math.max(0, down ? below : above)}px`;
      const bounds = popup.getBoundingClientRect();
      popup.style.left = `${Math.max(minLeft, Math.min(anchor.left, maxRight - bounds.width))}px`;
      popup.style.top = `${Math.max(top + 8, down ? anchor.bottom + 6 : anchor.top - bounds.height - 6)}px`;
      popup.style.visibility = "visible";
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    window.visualViewport?.addEventListener("resize", place);
    window.visualViewport?.addEventListener("scroll", place);
    const resize = new ResizeObserver(place);
    if (field.current) resize.observe(field.current);
    if (input.current?.form) resize.observe(input.current.form);
    return () => {
      resize.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      window.visualViewport?.removeEventListener("resize", place);
      window.visualViewport?.removeEventListener("scroll", place);
    };
  }, [open, view]);

  useLayoutEffect(() => {
    if (open && focusDay.current) {
      const selector = view === "days" ? `[data-date="${active}"]` : `[data-period="${activePeriod}"]`;
      panel.current?.querySelector<HTMLButtonElement>(selector)?.focus({ preventScroll: true });
      focusDay.current = false;
    }
  }, [open, active, view, activePeriod]);

  useEffect(() => {
    if (!open) return;
    const dismiss = (event: Event) => {
      const target = event.target as Node;
      if (!field.current?.contains(target) && !panel.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("focusin", dismiss);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("focusin", dismiss);
    };
  }, [open]);

  function move(next: string, focus = false) {
    if (parseCalendarInput(next) === null) return;
    focusDay.current = focus;
    setActive(next);
  }

  function gridKey(event: KeyboardEvent<HTMLTableElement>) {
    const offset = (calendarDate(active).getUTCDay() - calendarWeekStart(locale) + 7) % 7;
    const next = event.key === "ArrowLeft" ? moveCalendarDays(active, -1)
      : event.key === "ArrowRight" ? moveCalendarDays(active, 1)
      : event.key === "ArrowUp" ? moveCalendarDays(active, -7)
      : event.key === "ArrowDown" ? moveCalendarDays(active, 7)
      : event.key === "Home" ? moveCalendarDays(active, -offset)
      : event.key === "End" ? moveCalendarDays(active, 6 - offset)
      : event.key === "PageUp" ? moveCalendarMonths(active, event.shiftKey ? -12 : -1)
      : event.key === "PageDown" ? moveCalendarMonths(active, event.shiftKey ? 12 : 1) : null;
    if (next) { event.preventDefault(); move(next, true); }
  }

  function changeView(next: typeof view) {
    focusDay.current = true;
    setView(next);
  }

  function choosePeriod(period: number) {
    move(moveCalendarMonths(active, view === "months" ? period - activeMonth : (period - activeYear) * 12), true);
    setView(view === "months" ? "days" : "months");
  }

  function periodKey(event: KeyboardEvent<HTMLTableElement>) {
    const index = periods.findIndex(period => period.value === activePeriod);
    const delta = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1
      : event.key === "ArrowUp" ? -3 : event.key === "ArrowDown" ? 3
      : event.key === "Home" ? -(index % 3) : event.key === "End" ? 2 - index % 3
      : event.key === "PageUp" ? -12 : event.key === "PageDown" ? 12 : null;
    if (delta !== null) { event.preventDefault(); move(moveCalendarMonths(active, delta * (view === "months" ? 1 : 12)), true); }
  }

  const popup = open && <div ref={panel} id={`${id}-calendar`} className="date-calendar" role="dialog" aria-label={t("calendar.chooseDate", { label })}
    onKeyDown={event => {
      if (event.key === "Escape") {
        event.preventDefault(); event.stopPropagation();
        if (view !== "days") changeView(view === "years" ? "months" : "days");
        else close(true);
      }
      if (event.key !== "Tab") return;
      const stops = panel.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled):not([tabindex="-1"])');
      if (!stops?.length) return;
      if (event.shiftKey && event.target === stops[0]) { event.preventDefault(); close(true); }
      else if (!event.shiftKey && event.target === stops[stops.length - 1]) {
        const controls = Array.from(input.current?.form?.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href]') ?? []);
        const next = controls[controls.indexOf(trigger.current!) + 1];
        event.preventDefault(); close(); (next ?? trigger.current)?.focus({ preventScroll: true });
      }
    }}>
    <div className="date-calendar-heading">
      <button type="button" className="date-calendar-period" aria-label={t(view === "days" ? "calendar.chooseMonthYear" : view === "months" ? "calendar.chooseYear" : "calendar.backToMonths", { period: heading, year: activeYear })}
        aria-expanded={view !== "days"} aria-controls={`${id}-body`} onClick={() => changeView(view === "days" ? "months" : view === "months" ? "years" : "months")}>
        <span id={`${id}-month`} aria-live="polite" aria-atomic="true">{heading}</span>{view === "years" ? <ChevronUp /> : <ChevronDown />}
      </button>
      <div className="date-calendar-navigation">
        <button type="button" aria-label={t(view === "days" ? "calendar.previousMonth" : view === "months" ? "calendar.previousYear" : "calendar.previousYears")} disabled={parseCalendarInput(moveCalendarMonths(active, -step)) === null} onClick={() => move(moveCalendarMonths(active, -step))}><ChevronLeft /></button>
        <button type="button" aria-label={t(view === "days" ? "calendar.nextMonth" : view === "months" ? "calendar.nextYear" : "calendar.nextYears")} disabled={parseCalendarInput(moveCalendarMonths(active, step)) === null} onClick={() => move(moveCalendarMonths(active, step))}><ChevronRight /></button>
      </div>
    </div>
    <div className="date-calendar-body" id={`${id}-body`}>
    {view === "days" ? <table role="grid" aria-labelledby={`${id}-month`} onKeyDown={gridKey}>
      <thead><tr>{weekdays.map(day => <th key={day} scope="col">{day}</th>)}</tr></thead>
      <tbody>{Array.from({ length: 6 }, (_, week) => <tr key={week}>{days.slice(week * 7, week * 7 + 7).map(day => <td key={day} aria-selected={day === value}>
        <button type="button" data-date={day} data-outside={day.slice(0, 7) !== active.slice(0, 7) || undefined} data-selected={day === value || undefined}
          tabIndex={day === active ? 0 : -1} aria-label={dayLabel.format(calendarDate(day))} aria-current={day === today ? "date" : undefined}
          disabled={parseCalendarInput(day) === null} onClick={() => choose(day)}>{calendarDate(day).getUTCDate()}</button>
      </td>)}</tr>)}</tbody>
    </table> : <table className="date-period-grid" role="grid" aria-label={view === "months" ? t("calendar.monthsOfYear", { year: activeYear }) : t("calendar.yearRange", { start: yearStart, end: yearStart + 11 })} onKeyDown={periodKey}>
      <tbody>{Array.from({ length: 4 }, (_, row) => <tr key={row}>{periods.slice(row * 3, row * 3 + 3).map(period => <td key={period.value} aria-selected={period.value === activePeriod}>
        <button type="button" data-period={period.value} data-selected={period.value === activePeriod || undefined} tabIndex={period.value === activePeriod ? 0 : -1}
          onClick={() => choosePeriod(period.value)}>{period.label}</button>
      </td>)}</tr>)}</tbody>
    </table>}
    </div>
    <div className="date-calendar-footer">
      <button type="button" disabled={!text} onClick={() => choose("")}>{t("calendar.clearDate")}</button>
      <button type="button" onClick={() => choose(today)}>{t("period.today")}</button>
    </div>
  </div>;

  return <div className="date-field">
    <label htmlFor={id}>{label}</label>
    <div className="date-control" ref={field} onKeyDown={event => {
      if (event.key === "Escape" && open) { event.preventDefault(); event.stopPropagation(); close(true); }
    }}>
      <input id={id} ref={input} type="text" inputMode="numeric" autoComplete="off" placeholder={t("calendar.inputFormat")} value={text}
        aria-invalid={invalid || undefined} aria-describedby={invalid ? `${id}-error` : undefined}
        onChange={event => {
          const draft = event.target.value, parsed = parseCalendarInput(draft, locale);
          setText(draft); setInvalid(false);
          event.target.setCustomValidity(parsed === null ? invalidDateMessage : "");
          if (parsed !== null) { emitted.current = parsed; onChange(parsed); }
        }}
        onInvalid={() => setInvalid(true)}
        onBlur={() => {
          const parsed = parseCalendarInput(text, locale);
          setInvalid(parsed === null);
          if (parsed !== null) setText(displayCalendarDate(parsed, locale));
        }}
        onKeyDown={event => {
          if (event.key === "ArrowDown") { event.preventDefault(); show(); }
        }} />
      <button ref={trigger} type="button" className="date-toggle" aria-label={t("calendar.open", { label })} aria-haspopup="dialog"
        aria-expanded={open} aria-controls={open ? `${id}-calendar` : undefined} onClick={() => open ? close() : show()}><Calendar /></button>
    </div>
    {invalid && <span className="date-error" id={`${id}-error`} role="alert">{invalidDateMessage}</span>}
    {popup && createPortal(popup, field.current?.closest(".relay-app") ?? document.body)}
  </div>;
}
