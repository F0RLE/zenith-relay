import { useId, useState } from "react";
import type { CSSProperties, KeyboardEvent, PointerEvent } from "react";
import type { IconComponent } from "../../components/Icons";
import type { RequestRecord } from "../usage/model";
import type { OverviewBucket } from "./timeline";
import { chartCeiling, chartLinePath } from "./chart-display";
import { useLocale } from "../../i18n/LocaleProvider";
import { useFormatters } from "../../i18n/format";

export interface ChartSeries { label: string; measure: (rows: RequestRecord[]) => number | null; tone: string; percentage?: boolean }

// Hosted presentation only. Values and UTC buckets come from the existing model.
export function Chart({ buckets, measure, icon: Icon, label, total, hint, note, unit, tone, mode = "bar", series }: {
  buckets: OverviewBucket[]; measure: (rows: RequestRecord[]) => number | null; icon: IconComponent;
  label: string; total: string; hint?: string; note?: string; unit: "tokens" | "requests" | "tok/s" | "USD"; tone: string;
  mode?: "bar" | "line"; series?: ChartSeries[];
}) {
  const { t, locale } = useLocale();
  const { formatCompact, formatPercent } = useFormatters();
  const unitKeys = { tokens: "chart.tokens", requests: "chart.requests", "tok/s": "chart.tokensPerSecond" } as const;
  const unitLabel = unit === "USD" ? unit : t(unitKeys[unit]);
  const id = useId();
  const [active, setActive] = useState<number | null>(null);
  const [hidden, setHidden] = useState<string[]>([]);
  const allSeries = series ?? [{ label, measure, tone }];
  const points = allSeries.filter(item => !hidden.includes(item.tone)).map(item => ({ ...item, values: buckets.map(bucket => item.measure(bucket.rows)) }));
  const ceiling = chartCeiling(points.filter(item => !item.percentage).flatMap(item => item.values), unit);
  const short = (value: number) => unit === "USD" ? value.toLocaleString(locale, { maximumFractionDigits: 6 }) : formatCompact(value);
  const axis = [short(ceiling), short(ceiling / 2), "0"];
  const stem = series ? "overview-token-trend" : "overview-chart";
  const measured = points.some(item => item.values.some(value => value !== null));
  const bucket = active === null ? undefined : buckets[active];
  const rate = points.some(item => item.percentage);
  const primary = points.some(item => !item.percentage);
  const bar = mode === "bar" && !series;
  const lastMeasured = buckets.reduce((last, _, index) => points.some(item => item.values[index] !== null) ? index : last, 0);
  function pointAt(event: PointerEvent<HTMLDivElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    setActive(Math.max(0, Math.min(buckets.length - 1, Math.floor((event.clientX - rect.left) / rect.width * buckets.length))));
  }
  function navigate(event: KeyboardEvent<HTMLDivElement>) {
    const current = active ?? lastMeasured;
    if (event.key === "Escape") { setActive(null); event.stopPropagation(); return; }
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    setActive(event.key === "Home" ? 0 : event.key === "End" ? buckets.length - 1
      : Math.max(0, Math.min(buckets.length - 1, current + (event.key === "ArrowRight" ? 1 : -1))));
  }
  return <article className={`overview-chart hosted-chart ${tone}${series ? " overview-token-trend" : ""}`} aria-label={label}>
    <header className="hosted-chart-header">
      <h2 className="hosted-chart-title"><Icon aria-hidden="true" />{label}</h2>
      <div className={`hosted-chart-total${series ? " overview-token-trend-summary" : ""}`}>
        <strong className="overview-chart-summary" data-relay-tooltip={hint} tabIndex={hint ? 0 : undefined}>{total}</strong>
        {note && <small>{note}</small>}
      </div>
    </header>
    {series && <div className="hosted-chart-legend" role="group" aria-label={t("chart.tokenMetrics")}>
      {series.map(item => <button key={item.tone} type="button" className={`is-${item.tone}`} aria-pressed={!hidden.includes(item.tone)}
        onClick={() => { setHidden(previous => previous.includes(item.tone) ? previous.filter(value => value !== item.tone) : [...previous, item.tone]); setActive(null); }}>
        <i aria-hidden="true" />{item.label}
      </button>)}
    </div>}
    <div className="hosted-chart-body" data-rate-axis={rate} data-primary-axis={primary}>
      {primary && <div className="hosted-chart-axis" aria-hidden="true">{axis.map((value, index) => <span key={index}>{value}</span>)}</div>}
      <div className={`${stem}-plot hosted-chart-plot`}>
        <div className="hosted-chart-canvas" role="group" aria-label={t("chart.timeChart", { label })} tabIndex={0}
          aria-describedby={`${id}-instructions${bucket && points.length ? ` ${id}-tooltip` : ""}`}
          onPointerMove={pointAt} onPointerDown={pointAt}
          onPointerLeave={event => { if (event.pointerType !== "touch" && document.activeElement !== event.currentTarget) setActive(null); }}
          onFocus={() => setActive(current => current ?? lastMeasured)} onBlur={() => setActive(null)} onKeyDown={navigate}>
          <svg role="img" aria-label={t("chart.accessibleDescription", { label, unit: unitLabel })} viewBox="0 0 100 100" preserveAspectRatio="none">
            <path className="hosted-chart-grid" d="M0 0H100 M0 50H100 M0 100H100" />
            {!bar && points.map(item => <path key={item.tone} className={`hosted-chart-line is-${item.tone}`} d={chartLinePath(item.values, item.percentage ? 100 : ceiling)} />)}
          </svg>
          <ol className="hosted-chart-points" aria-hidden="true" style={{ gridTemplateColumns: `repeat(${buckets.length}, minmax(0, 1fr))` }}>
            {buckets.map((item, index) => <li key={item.startMs} data-active={active === index}>
              {points.map(point => {
                const value = point.values[index];
                if (value === null || value === undefined) return null;
                const ratio = value / (point.percentage ? 100 : ceiling);
                const isolated = point.values[index - 1] == null && point.values[index + 1] == null;
                return <span key={point.tone} className={`hosted-chart-mark ${bar ? "hosted-chart-bar" : "hosted-chart-dot"} is-${point.tone}`}
                  data-isolated={isolated} data-last={index === lastMeasured} data-interval={item.interval} data-value={value}
                  style={(bar ? { height: `${ratio * 100}%` } : { top: `${(1 - ratio) * 100}%` }) as CSSProperties} />;
              })}
            </li>)}
          </ol>
          {!measured && <span className="hosted-chart-empty">{t(points.length ? "chart.noMeasurements" : "chart.chooseAMetric")}</span>}
        </div>
      </div>
      {rate && <div className="hosted-chart-axis hosted-chart-rate-axis" aria-hidden="true">{[1, .5, 0].map(value => <span key={value}>{formatPercent(value, 0)}</span>)}</div>}
      {bucket && points.length > 0 && <div id={`${id}-tooltip`} className="hosted-chart-tooltip" role="tooltip" aria-live="polite" aria-atomic="true" data-side={active! >= buckets.length / 2 ? "start" : "end"}>
        <span className="hosted-chart-tooltip-time">{bucket.interval}</span>
        <dl>{points.map(item => <div key={item.tone} className={`is-${item.tone}`}>
          <dt><i aria-hidden="true" />{item.label}</dt>
          <dd>{item.values[active!] === null ? "—" : item.percentage ? formatPercent(item.values[active!]! / 100) : `${item.values[active!]!.toLocaleString(locale, { maximumFractionDigits: unit === "tok/s" ? 2 : 6 })}${unit === "tokens" || unit === "requests" ? "" : ` ${unitLabel}`}`}</dd>
        </div>)}</dl>
      </div>}
    </div>
    <span id={`${id}-instructions`} className="sr-only">{t("chart.keyboardHelp")}</span>
  </article>;
}
