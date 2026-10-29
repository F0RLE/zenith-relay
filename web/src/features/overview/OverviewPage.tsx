import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { Activity, CircleAlert, Database, Gauge, Radio, SlidersHorizontal, Wallet } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { OptionMenu } from "../../components/OptionMenu";
import { formatInteger, formatMoney, formatSpeed, generationSpeed, usageTotals } from "../usage/model";
import type { RequestRecord } from "../usage/model";
import { formatOverviewInterval, overviewTimeline } from "./timeline";
import type { OverviewBucket, OverviewRange } from "./timeline";

interface Series { label: string; measure: (rows: RequestRecord[]) => number | null; color: string; percentage?: boolean }
function compactNumber(value: number) {
  const divisor = value >= 1e6 ? 1e6 : value >= 1e3 ? 1e3 : 1;
  return `${(value / divisor).toLocaleString("ru-RU", { maximumFractionDigits: divisor === 1 ? 2 : 1 })}${divisor === 1e6 ? " млн" : divisor === 1e3 ? " тыс." : ""}`;
}
function Chart({ buckets, measure, label, unit, color, mode = "bar", series }: {
  buckets: OverviewBucket[]; measure: (rows: RequestRecord[]) => number | null; label: string; unit: string; color: string; mode?: "bar" | "line"; series?: Series[];
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  useEffect(() => {
    const observer = new ResizeObserver(([entry]) => { if (entry && entry.contentRect.width > 0) setWidth(entry.contentRect.width); });
    observer.observe(ref.current!);
    return () => observer.disconnect();
  }, []);
  const left = 64, right = 40, top = 12, bottom = 26, height = series ? 174 : 164;
  const values = buckets.map(bucket => measure(bucket.rows));
  const activeSeries = series ?? [{ label, measure, color }];
  const maximumValue = Math.max(0, ...activeSeries.filter(item => !item.percentage).flatMap(item => buckets.map(bucket => item.measure(bucket.rows) ?? 0)));
  const scale = 10 ** Math.floor(Math.log10(maximumValue || 1));
  const tick = [1, 2, 4, 6, 8, 10].find(value => value >= maximumValue / scale) ?? 10;
  const maximum = Math.max(unit === "запросы" ? 2 : 0, tick * scale);
  const plotWidth = Math.max(1, width - left - right), plotHeight = height - top - bottom;
  const x = (index: number) => left + (index + .5) * plotWidth / buckets.length;
  const y = (value: number) => top + plotHeight - value / maximum * plotHeight;
  const short = (value: number) => unit === "USD" ? value.toLocaleString("ru-RU", { maximumFractionDigits: 6 }) : compactNumber(unit === "запросы" ? Math.round(value) : value);
  const step = Math.max(1, Math.ceil((buckets.length - 1) / (width < 450 ? 2 : 5)));
  function line(item: Series) {
    let drawing = "", connected = false;
    const points: { x: number; y: number; label: string }[] = [];
    buckets.forEach((bucket, index) => {
      const value = item.measure(bucket.rows);
      if (value === null) { connected = false; return; }
      const pointY = item.percentage ? top + plotHeight - value / 100 * plotHeight : y(value);
      drawing += `${connected ? "L" : "M"}${x(index)},${pointY} `;
      connected = true;
      const measured = unit === "ток/с" ? ` · измерено ${bucket.rows.filter(row => generationSpeed(row) !== null).length} из ${bucket.rows.length} запросов` : "";
      points.push({ x: x(index), y: pointY, label: `${bucket.interval}${bucket.partial ? " · неполный интервал" : ""} · ${item.label}: ${value.toLocaleString("ru-RU", { maximumFractionDigits: 6 })} ${item.percentage ? "%" : unit}${measured}` });
    });
    return <g key={item.label}><path d={drawing} fill="none" stroke={item.color} strokeWidth={1.6} strokeDasharray={item.percentage ? "4 3" : undefined} />{points.map(point => <circle key={point.label} cx={point.x} cy={point.y} r={2.3} stroke={item.color} strokeWidth={1.6} fill="var(--surface)"><title>{point.label}</title></circle>)}</g>;
  }
  return <div className="overview-chart" ref={ref} style={{ maxWidth: buckets.length * 96 + left + right }}>
    {series && <div className="chart-legend">{series.map(item => <span key={item.label} style={{ color: item.color }}><i />{item.label}</span>)}</div>}
    <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${label}, ${unit}. Точные значения доступны в таблице данных.`}>
      {[0, .5, 1].map(factor => <g key={factor}><line x1={left} x2={width - right} y1={y(maximum * factor)} y2={y(maximum * factor)} className="chart-grid" /><text x={0} y={y(maximum * factor) + 3}>{short(maximum * factor)}</text></g>)}
      {series && [0, 50, 100].map(value => <text className="chart-percentage" key={value} x={width - right + 7} y={top + plotHeight - value / 100 * plotHeight + 3}>{value}%</text>)}
      {(mode === "line" || series) && activeSeries.map(line)}
      {buckets.map((bucket, index) => {
        const value = values[index];
        const barWidth = Math.min(24, plotWidth / buckets.length * .45);
        return <g key={bucket.label}>
          {mode === "bar" && !series && value !== null && value !== undefined && value > 0 && <rect x={x(index) - barWidth / 2} y={y(value)} width={barWidth} height={height - bottom - y(value)} rx={2} fill={color} fillOpacity={.72}><title>{bucket.interval}{bucket.partial ? " · неполный интервал" : ""}: {value.toLocaleString("ru-RU", { maximumFractionDigits: 6 })} {unit}</title></rect>}
          {(index % step === 0 || index === buckets.length - 1) && <text x={x(index)} y={height - 5} textAnchor={index === 0 ? "start" : index === buckets.length - 1 ? "end" : "middle"}>{bucket.label}</text>}
        </g>;
      })}
    </svg>
  </div>;
}

export function OverviewPage({ records, asOf }: { records: readonly RequestRecord[]; asOf: string }) {
  const [range, setRange] = useState<OverviewRange>("today");
  const [model, setModel] = useState("all");
  const { rows, buckets, startMs, endMs } = overviewTimeline(records, range, asOf, model);
  const interval = range === "today" ? "1 час" : "1 день";
  const summary = usageTotals(rows);
  const measured = rows.filter(row => generationSpeed(row) !== null).length;
  const models = [...new Map(records.map(row => [row.model.id, row.model])).values()];
  const metrics: [LucideIcon, string, number][] = [[Activity, "Запросы", rows.length], [Radio, "В работе сейчас", records.filter(row => row.execution === "running").length], [SlidersHorizontal, "Модели", models.length], [CircleAlert, "Ошибки", rows.filter(row => row.execution === "failed").length]];
  const sumTokens = (values: RequestRecord[], field: "input" | "output" | "cacheRead" | "cacheWrite") => values.every(row => row.tokens[field] === null) ? null : values.reduce((sum, row) => sum + (row.tokens[field] ?? 0), 0);
  const input = sumTokens(rows, "input"), cache = sumTokens(rows, "cacheRead");
  const cacheHit = input && cache !== null ? cache / input * 100 : null;
  const tokenSeries: Series[] = [
    { label: "Вход", color: "var(--chart-input)", measure: values => sumTokens(values, "input") },
    { label: "Выход", color: "var(--chart-output)", measure: values => sumTokens(values, "output") },
    { label: "Запись кэша", color: "var(--chart-cache-write)", measure: values => sumTokens(values, "cacheWrite") },
    { label: "Чтение кэша", color: "var(--chart-cache)", measure: values => sumTokens(values, "cacheRead") },
    { label: "Попадания в кэш", color: "var(--chart-e2e)", percentage: true, measure: values => { const input = sumTokens(values, "input"), cache = sumTokens(values, "cacheRead"); return input && cache !== null ? cache / input * 100 : null; } },
  ];
  const charts: { icon: LucideIcon; label: string; total: string; note?: string; detail?: string; definition?: string; unit: string; color: string; mode?: "bar" | "line"; series?: Series[]; measure: (rows: RequestRecord[]) => number | null }[] = [
    { icon: Database, label: "Использование токенов", total: summary.known ? compactNumber(summary.tokens) : rows.length ? "—" : "0", detail: `${formatInteger(summary.tokens)} токенов · ${summary.known} из ${summary.count} запросов с данными`, note: cacheHit === null ? "Попадания в кэш —" : `${cacheHit.toLocaleString("ru-RU", { maximumFractionDigits: 1 })}% попадания в кэш`, unit: "токены", color: "var(--accent)", series: tokenSeries, measure: values => { const totals = usageTotals(values); return totals.known ? totals.tokens : values.length ? null : 0; } },
    { icon: Wallet, label: "Расходы", total: `${formatMoney(summary.charge)} USD`, ...(summary.pending ? { note: `${summary.pending} без итогового расчёта` } : {}), unit: "USD", color: "var(--chart-cost)", measure: values => { const amount = usageTotals(values).charge; return amount === null ? null : Number(amount) / 1000000; } },
    { icon: Activity, label: "Запросы", total: compactNumber(summary.count), detail: `${formatInteger(summary.count)} запросов`, unit: "запросы", color: "var(--info)", measure: values => values.length },
    { icon: Gauge, label: "Скорость генерации", total: formatSpeed(summary.generation), note: `Измерено ${measured} из ${rows.length} запросов`, definition: "Выходные токены / суммарное время генерации.", unit: "ток/с", color: "var(--chart-speed)", mode: "line", measure: values => usageTotals(values).generation },
    { icon: Gauge, label: "Скорость с учётом ожидания", total: formatSpeed(summary.e2e), note: `Измерено ${measured} из ${rows.length} запросов`, definition: "Выходные токены / полное время запроса, включая ожидание первого токена.", unit: "ток/с", color: "var(--chart-e2e)", mode: "line", measure: values => usageTotals(values).e2e },
  ];
  return <div className="overview-page">
    <header className="page-heading"><h1>Обзор</h1><span>Снимок: {new Date(asOf).toLocaleString("ru-RU", { timeZone: "UTC", day: "numeric", month: "long", hour: "2-digit", minute: "2-digit" })} UTC</span></header>
    <div className="overview-metrics">{metrics.map(([Icon, label, value]) => <div key={label}><Icon aria-hidden="true" /><span>{label}</span><strong>{formatInteger(value)}</strong></div>)}</div>
    <section className="overview-analytics" aria-labelledby="overview-statistics">
      <header><div className="overview-section-title"><h2 id="overview-statistics">Использование во времени</h2><span className="overview-period">{formatOverviewInterval(startMs, endMs)}</span><span className="overview-step">Точка на графике — {interval}.{buckets.at(-1)?.partial && " Последний интервал неполный."} Сводка справа — за весь период.</span></div><div className="overview-controls"><OptionMenu label="Модель статистики" value={model} onChange={setModel} options={[{ value: "all", label: "Все модели" }, ...models.map(item => ({ value: item.id, label: item.name }))]} /><div className="period-buttons" role="group" aria-label="Период статистики">{([["today", "Сегодня"], ["week", "7 дней"], ["month", "30 дней"]] as const).map(([id, text]) => <button key={id} type="button" aria-pressed={range === id} onClick={() => setRange(id)}>{text}</button>)}</div></div></header>
      {charts.map(({ icon: Icon, label, total, note, detail, definition, ...chart }) => <article className="overview-chart-block" key={label} aria-label={label} style={{ "--chart-color": chart.color } as CSSProperties}><div className="overview-chart-heading"><h3><Icon aria-hidden="true" />{label}</h3><div className="overview-chart-total"><strong title={detail}>{total}</strong>{note && <small className={chart.series ? "cache-hit-note" : undefined}>{note}</small>}</div></div>{definition && <p className="chart-definition">{definition}</p>}<Chart buckets={buckets} label={label} {...chart} />{chart.series && <p className="chart-coverage">Счётчики получены: {summary.known} из {summary.count} запросов</p>}</article>)}
      <div className="sr-only"><table><caption>Данные графиков</caption><thead><tr><th>Интервал, UTC</th><th>Запросы</th><th>Известные токены</th><th>Списано, USD</th><th>Скорость генерации</th><th>С учётом ожидания</th></tr></thead><tbody>{buckets.map(bucket => { const totals = usageTotals(bucket.rows); return <tr key={bucket.startMs}><td>{bucket.interval}{bucket.partial ? " · неполный интервал" : ""}</td><td>{totals.count}</td><td>{totals.known ? formatInteger(totals.tokens) : totals.count ? "—" : "0"}</td><td>{formatMoney(totals.charge)}</td><td>{formatSpeed(totals.generation)}</td><td>{formatSpeed(totals.e2e)}</td></tr>; })}</tbody></table></div>
    </section>
  </div>;
}
