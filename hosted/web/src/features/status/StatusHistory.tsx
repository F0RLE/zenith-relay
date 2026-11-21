import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useLocale } from "../../i18n/LocaleProvider";
import type { StatusInterval } from "../../api/modelStatus";
import type { StatusPeriod } from "./history";
import { StatusValue } from "./StatusValue";

export function StatusHistory({ modelName, intervals, period }: { modelName: string; intervals: readonly StatusInterval[]; period: StatusPeriod }) {
  const { t, locale } = useLocale();
  const id = useId();
  const strip = useRef<HTMLDivElement>(null);
  const tooltip = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [active, setActive] = useState<number | null>(null);
  const interval = active === null ? undefined : intervals[active];
  const dated = period === "week" || !!interval && Math.floor(interval.startMs / 86400000) !== Math.floor(interval.endMs / 86400000);
  const format = new Intl.DateTimeFormat(locale, { timeZone: "UTC", hour: "2-digit", minute: "2-digit", ...(dated ? { month: "short", day: "numeric" } as const : {}) });
  const range = interval ? `${format.format(interval.startMs)} – ${format.format(interval.endMs)} UTC` : "";
  function cancelClose() { if (closeTimer.current !== null) clearTimeout(closeTimer.current); }
  function deferClose() {
    cancelClose();
    if (document.activeElement !== strip.current) closeTimer.current = setTimeout(() => setActive(null), 120);
  }
  function selectAt(clientX: number) {
    cancelClose();
    const bounds = strip.current?.getBoundingClientRect();
    if (bounds?.width && intervals.length) setActive(Math.min(intervals.length - 1, Math.max(0, Math.floor((clientX - bounds.left) / bounds.width * intervals.length))));
  }
  useEffect(() => () => cancelClose(), []);
  useEffect(() => {
    if (active === null) return;
    const close = () => setActive(null);
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    const outside = (event: PointerEvent) => { if (!strip.current?.contains(event.target as Node) && !tooltip.current?.contains(event.target as Node)) close(); };
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", key);
    return () => { window.removeEventListener("resize", close); window.removeEventListener("scroll", close, true); document.removeEventListener("pointerdown", outside, true); document.removeEventListener("keydown", key); };
  }, [active]);
  useLayoutEffect(() => {
    if (active === null || !strip.current || !tooltip.current) return;
    const anchor = strip.current.getBoundingClientRect();
    const panel = tooltip.current.getBoundingClientRect();
    const x = anchor.left + (active + .5) / intervals.length * anchor.width;
    const above = anchor.top - panel.height - 10;
    tooltip.current.style.left = `${Math.max(8, Math.min(x - panel.width / 2, innerWidth - panel.width - 8))}px`;
    tooltip.current.style.top = `${Math.max(8, Math.min(above >= 8 ? above : anchor.bottom + 10, innerHeight - panel.height - 8))}px`;
    tooltip.current.style.visibility = "visible";
  }, [active, interval, intervals.length, locale]);

  return <>
    <div ref={strip} className="model-status-history" role="group" tabIndex={0} aria-label={t("modelStatus.historyFor", { name: modelName })} aria-describedby={`${id}-help${interval ? ` ${id}-tooltip` : ""}`} style={{ gridTemplateColumns: `repeat(${intervals.length || 1}, minmax(0, 1fr))` }}
      onPointerMove={event => { if (event.pointerType !== "touch") selectAt(event.clientX); }} onPointerDown={event => selectAt(event.clientX)} onPointerLeave={deferClose}
      onFocus={() => { cancelClose(); setActive(current => current ?? intervals.length - 1); }} onBlur={() => setActive(null)}
      onKeyDown={event => {
        if (event.key === "Escape") { event.preventDefault(); cancelClose(); setActive(null); }
        else if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
          event.preventDefault(); cancelClose();
          setActive(current => event.key === "Home" ? 0 : event.key === "End" ? intervals.length - 1 : Math.max(0, Math.min(intervals.length - 1, (current ?? intervals.length - 1) + (event.key === "ArrowLeft" ? -1 : 1))));
        }
      }}>
      {intervals.map((item, index) => <span key={item.startMs} data-state={item.metrics?.state ?? "unknown"} data-active={active === index} aria-hidden="true" />)}
    </div>
    <span id={`${id}-help`} className="sr-only">{t("modelStatus.historyHelp")}</span>
    {interval && createPortal(<div ref={tooltip} id={`${id}-tooltip`} className="model-status-tooltip" role="tooltip" aria-live="polite" aria-atomic="true" onPointerEnter={cancelClose} onPointerLeave={deferClose}>
      <strong>{range}</strong>
      <dl>{([["availability", "modelStatus.availability"], ["latency", interval.metrics ? "modelStatus.latency" : "modelStatus.firstToken"],
        ["requests", "common.requests"], ["cache", "modelStatus.cacheHitRate"]] as const).map(([metric, label]) =>
        <div key={metric}><dt>{t(label)}</dt><dd><StatusValue metric={metric} values={interval.metrics} interval /></dd></div>)}</dl>
    </div>, strip.current?.closest(".hosted-app") ?? document.body)}
  </>;
}
