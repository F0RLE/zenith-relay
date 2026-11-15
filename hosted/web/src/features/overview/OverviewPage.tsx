import { useState } from "react";
import { Activity, CreditCard, Database, Gauge, Timer } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { generationSpeed, usageTotals } from "../usage/model";
import type { RequestRecord } from "../usage/model";
import { formatOverviewInterval, overviewTimeline } from "./timeline";
import type { OverviewRange } from "./timeline";
import { Chart } from "./OverviewChart";
import type { ChartSeries } from "./OverviewChart";
import { useLocale } from "../../i18n/LocaleProvider";
import { useFormatters } from "../../i18n/format";

export function OverviewPage({ records, asOf, owner = false }: { records: readonly RequestRecord[]; asOf: string; owner?: boolean }) {
  const { t, locale } = useLocale();
  const { formatInteger, formatMoney, formatSpeed, formatCompact: compactNumber, formatPercent } = useFormatters();
  const [range, setRange] = useState<OverviewRange>("24h");
  const [model, setModel] = useState("all");
  const { rows, buckets, startMs, endMs } = overviewTimeline(records, range, asOf, model, locale);
  const summary = usageTotals(rows);
  const measured = rows.filter(row => generationSpeed(row) !== null).length;
  const models = [...new Map(records.map(row => [row.model.id, row.model])).values()];
  const sumTokens = (values: RequestRecord[], field: "input" | "output" | "cacheRead" | "cacheWrite") => values.every(row => row.tokens[field] === null) ? null : values.reduce((sum, row) => sum + (row.tokens[field] ?? 0), 0);
  const input = sumTokens(rows, "input"), cache = sumTokens(rows, "cacheRead");
  const cacheHit = input && cache !== null ? cache / input * 100 : null;
  const tokenSeries: ChartSeries[] = [
    { label: t("overview.input"), tone: "input", measure: values => sumTokens(values, "input") },
    { label: t("overview.output"), tone: "output", measure: values => sumTokens(values, "output") },
    { label: t("overview.cacheCreation"), tone: "cache-write", measure: values => sumTokens(values, "cacheWrite") },
    { label: t("overview.cacheReads"), tone: "cache-read", measure: values => sumTokens(values, "cacheRead") },
    { label: t("overview.cacheHits"), tone: "cache-rate", percentage: true, measure: values => { const input = sumTokens(values, "input"), cache = sumTokens(values, "cacheRead"); return input && cache !== null ? cache / input * 100 : null; } },
  ];
  return <section className="desktop-overview hosted-overview">
    <header className="page-heading overview-heading">
      <h1 id="overview-statistics" data-relay-tooltip={t("overview.intervalHint", { interval: formatOverviewInterval(startMs, endMs, locale) })} tabIndex={0}>{t("nav.overview")}</h1>
      <div className="overview-controls">
        <OptionMenu label={t("overview.statisticsModel")} value={model} onChange={setModel} options={[{ value: "all", label: t("usage.allModels") }, ...models.map(item => ({ value: item.id, label: item.name }))]} />
        <div className="relay-tabs" role="group" aria-label={t("overview.statisticsPeriod")}>
          {([["24h", "period.24Hours"], ["week", "period.7Days"], ["month", "period.30Days"]] as const).map(([id, text]) => <button key={id} type="button" className={range === id ? "active" : ""} aria-pressed={range === id} onClick={() => setRange(id)}>{t(text)}</button>)}
          {owner && <button type="button" className={range === "all" ? "active" : ""} aria-pressed={range === "all"} onClick={() => setRange("all")}>{t("period.allTime")}</button>}
        </div>
      </div>
    </header>
    <section className="overview-analytics has-data" aria-labelledby="overview-statistics">
      <div className="overview-chart-stack">
        <Chart buckets={buckets} icon={Database} label={t("overview.tokenUsage")} total={summary.known ? compactNumber(summary.tokens) : rows.length ? "—" : "0"} hint={t("overview.tokenCoverage", { tokens: formatInteger(summary.tokens), known: formatInteger(summary.known), count: formatInteger(summary.count) })} note={t("overview.cacheHitsRate", { rate: cacheHit === null ? "—" : formatPercent(cacheHit / 100, 0) })} unit="tokens" tone="tokens" series={tokenSeries} measure={values => { const totals = usageTotals(values); return totals.known ? totals.tokens : values.length ? null : 0; }} />
        <Chart buckets={buckets} icon={CreditCard} label={t(owner ? "overview.apiCharges" : "overview.expenses")} total={`${formatMoney(summary.charge)} USD`} hint={summary.pending ? t("usage.unsettledCount", { count: formatInteger(summary.pending) }) : owner ? t("overview.customerChargesHint") : t("overview.finalChargesForThePeriod")} unit="USD" tone="cost" measure={values => { const amount = usageTotals(values).charge; return amount === null ? null : Number(amount) / 1000000; }} />
        <Chart buckets={buckets} icon={Activity} label={t("common.requests")} total={compactNumber(summary.count)} unit="requests" tone="requests" measure={values => values.length} />
        <Chart buckets={buckets} icon={Gauge} label={t("usage.generationSpeed")} total={formatSpeed(summary.generation)} hint={t("overview.generationHint", { measured: formatInteger(measured), count: formatInteger(rows.length) })} unit="tok/s" tone="speed" mode="line" measure={values => usageTotals(values).generation} />
        <Chart buckets={buckets} icon={Timer} label={t("overview.speedIncludingWaitTime")} total={formatSpeed(summary.e2e)} hint={t("overview.totalSpeedHint", { measured: formatInteger(measured), count: formatInteger(rows.length) })} unit="tok/s" tone="e2e-speed" mode="line" measure={values => usageTotals(values).e2e} />
      </div>
      <div className="sr-only"><table><caption>{t("overview.chartData")}</caption><thead><tr>{(["overview.intervalUtc", "common.requests", "overview.knownTokens", "usage.chargedUsd", "usage.generationSpeed", "usage.includingWaitTime"] as const).map(label => <th key={label}>{t(label)}</th>)}</tr></thead><tbody>{buckets.map(bucket => { const totals = usageTotals(bucket.rows); return <tr key={bucket.startMs}><td>{bucket.interval}</td><td>{formatInteger(totals.count)}</td><td>{totals.known ? formatInteger(totals.tokens) : totals.count ? "—" : "0"}</td><td>{formatMoney(totals.charge)}</td><td>{formatSpeed(totals.generation)}</td><td>{formatSpeed(totals.e2e)}</td></tr>; })}</tbody></table></div>
    </section>
  </section>;
}
