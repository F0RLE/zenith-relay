import { useState } from "react";
import { Search } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { CustomerModel } from "../models/ModelsPage";
import { StatusHistory } from "./StatusHistory";
import { statusIntervals } from "./history";
import type { StatusPeriod } from "./history";
import "./status.css";

const metrics = [
  { id: "availability", label: "modelStatus.availability", column: "modelStatus.availability" },
  { id: "requests", label: "common.requests", column: "common.requests" },
  { id: "latency", label: "modelStatus.latency", column: "modelStatus.latency" },
  { id: "cache", label: "modelStatus.cacheHitRate", column: "modelStatus.cache" },
] as const;
const columns = [metrics[0], metrics[2], metrics[1], metrics[3]] as const;

/** Catalog membership is not evidence of service health or traffic. */
export function StatusPage({ models = [], asOf }: { models?: readonly CustomerModel[]; asOf?: string }) {
  const { t, locale } = useLocale();
  const [period, setPeriod] = useState<StatusPeriod>("day");
  const [query, setQuery] = useState("");
  const [openedAt] = useState(Date.now);
  const suppliedAt = asOf ? Date.parse(asOf) : NaN;
  const intervals = statusIntervals(period, Number.isFinite(suppliedAt) ? suppliedAt : openedAt);
  const search = query.trim().toLocaleLowerCase(locale);
  const rows = models.filter(model => `${model.name} ${model.id}`.toLocaleLowerCase(locale).includes(search));

  return <section className="status-page" data-period={period}>
    <header className="page-heading status-heading"><h1>{t("common.status")}</h1>
      <div className="relay-tabs model-status-period" role="group" aria-label={t("usage.periodUtc")}>
        {([ ["day", "period.24Hours"], ["week", "period.7Days"] ] as const).map(([id, label]) => <button type="button" key={id} className={period === id ? "active" : ""} aria-pressed={period === id} onClick={() => setPeriod(id)}>{t(label)}</button>)}
      </div>
    </header>
    <dl className="model-status-summary" aria-label={t("modelStatus.summary")}>
      {metrics.map(metric => <div key={metric.id}><dt>{t(metric.label)}</dt><dd><span aria-hidden="true">—</span><span className="sr-only">{t("modelStatus.noData")}</span></dd></div>)}
    </dl>

    <section className="model-status-models" aria-labelledby="status-models-title">
      <div className="model-status-toolbar">
        <h2 id="status-models-title">{t("nav.models")}<span>{rows.length.toLocaleString(locale)}</span></h2>
        <ul className="model-status-legend" aria-label={t("modelStatus.legend")}>
          {([ ["operational", "modelStatus.operational"], ["degraded", "modelStatus.degraded"], ["unavailable", "modelStatus.unavailable"], ["unknown", "modelStatus.noData"] ] as const).map(([state, label]) => <li key={state}><i className="model-status-dot" data-state={state} aria-hidden="true" />{t(label)}</li>)}
        </ul>
        <label className="model-status-search"><Search aria-hidden="true" /><span className="sr-only">{t("modelStatus.search")}</span><input type="search" value={query} placeholder={t("modelStatus.search")} onChange={event => setQuery(event.currentTarget.value)} /></label>
      </div>
      <div className="table-shell model-status-frame">
        <table className="model-status-table">
          <caption className="sr-only">{t("modelStatus.table")}</caption>
          <colgroup><col className="model-status-name-col" /><col className="model-status-history-col" />{columns.map(metric => <col key={metric.id} />)}</colgroup>
          <thead><tr><th scope="col">{t("common.model")}</th><th scope="col"><div className="model-status-axis"><span>{t(period === "day" ? "modelStatus.dayAgo" : "modelStatus.weekAgo")}</span><span>{t("modelStatus.now")}</span></div></th>{columns.map(metric => <th key={metric.id} scope="col">{t(metric.column)}</th>)}</tr></thead>
          <tbody>{rows.length ? rows.map(model => <tr key={model.id} className="model-status-row" data-state="unknown">
            <th scope="row"><strong className="model-status-name">{model.name}</strong></th>
            <td className="model-status-history-cell"><div className="model-status-axis model-status-mobile-axis" aria-hidden="true"><span>{t(period === "day" ? "modelStatus.dayAgo" : "modelStatus.weekAgo")}</span><span>{t("modelStatus.now")}</span></div><StatusHistory key={period} modelName={model.name} intervals={intervals} period={period} /></td>
            {columns.map(metric => <td className="model-status-value" key={metric.id} data-metric={metric.id}><span className="model-status-cell-label" aria-hidden="true">{t(metric.column)}</span><span aria-hidden="true">—</span><span className="sr-only">{t("modelStatus.noData")}</span></td>)}
          </tr>) : <tr className="model-status-empty"><td colSpan={6}><span role="status">{t(models.length ? "modelStatus.noMatches" : "modelStatus.noModels")}</span></td></tr>}</tbody>
        </table>
      </div>
    </section>
  </section>;
}
