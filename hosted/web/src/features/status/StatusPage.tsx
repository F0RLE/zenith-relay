import { useState } from "react";
import { statusStates, type StatusInterval, type StatusMetrics, type StatusState } from "../../api/modelStatus";
import { RefreshCw, Search } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { CustomerModel } from "../models/ModelsPage";
import { StatusHistory } from "./StatusHistory";
import { StatusValue } from "./StatusValue";
import { statusIntervals } from "./history";
import type { StatusPeriod } from "./history";
import { useModelStatus } from "./useModelStatus";
import "./status.css";

const metrics = [
  { id: "availability", label: "modelStatus.availability", column: "modelStatus.availability" },
  { id: "requests", label: "common.requests", column: "common.requests" },
  { id: "latency", label: "modelStatus.latency", column: "modelStatus.latency" },
  { id: "cache", label: "modelStatus.cacheHitRate", column: "modelStatus.cache" },
] as const;
const columns = [metrics[0], metrics[2], metrics[1], metrics[3]] as const;

interface Row { id: string; name: string; metrics?: StatusMetrics; intervals: StatusInterval[] }

export function StatusPage({ models = [], asOf, live = false }: { models?: readonly CustomerModel[]; asOf?: string; live?: boolean }) {
  const { t, locale } = useLocale();
  const [period, setPeriod] = useState<StatusPeriod>("day");
  const [query, setQuery] = useState("");
  const [openedAt] = useState(Date.now);
  const suppliedAt = asOf ? Date.parse(asOf) : NaN;
  const { result, refresh } = useModelStatus(live, period);
  const data = result?.data;
  const pending = live && !data && !result?.failed;
  const all: Row[] = live ? data?.items.map(row => ({ ...row, id: row.model, name: row.model })) ?? []
    : models.map(model => ({ id: model.id, name: model.name,
      intervals: statusIntervals(period, Number.isFinite(suppliedAt) ? suppliedAt : openedAt) }));
  const search = query.trim().toLocaleLowerCase(locale);
  const rows = all.filter(model => `${model.name} ${model.id}`.toLocaleLowerCase(locale).includes(search));
  function select(id: StatusPeriod) {
    if (id === period) return;
    setPeriod(id);
    if (live) refresh();
  }
  const tabs = <div className="relay-tabs model-status-period" role="group" aria-label={t("usage.periodUtc")}>
    {([ ["day", "period.24Hours"], ["week", "period.7Days"] ] as const).map(([id, label]) => <button type="button" key={id}
      className={period === id ? "active" : ""} aria-pressed={period === id} onClick={() => select(id)}>{t(label)}</button>)}
  </div>;

  return <section className="status-page" data-period={period}>
    <header className="page-heading status-heading"><h1>{t("common.status")}</h1>
      {live ? <div className="model-status-controls">{tabs}<button type="button" className="icon-button report-refresh"
        aria-label={t("modelStatus.refresh")} title={t("modelStatus.refresh")} disabled={!data} onClick={refresh}><RefreshCw aria-hidden="true" /></button></div> : tabs}
    </header>
    {pending || result?.failed ? <div className="table-shell model-status-placeholder">
      {pending ? <span role="status">{t("unavailable.connecting")}</span> : <>
        <span role="alert">{t("unavailable.server")}</span><button type="button" className="button" onClick={refresh}>{t("common.retry")}</button>
      </>}
    </div> : <>
    <dl className="model-status-summary" aria-label={t("modelStatus.summary")}>
      {metrics.map(metric => <div key={metric.id}><dt>{t(metric.label)}</dt><dd><StatusValue metric={metric.id} values={data?.summary} /></dd></div>)}
    </dl>

    <section className="model-status-models" aria-labelledby="status-models-title">
      <div className="model-status-toolbar">
        <h2 id="status-models-title">{t("nav.models")}<span>{rows.length.toLocaleString(locale)}</span></h2>
        <ul className="model-status-legend" aria-label={t("modelStatus.legend")}>
          {(Object.keys(statusStates) as StatusState[]).map(state => <li key={state}><i className="model-status-dot" data-state={state} aria-hidden="true" />{t(statusStates[state])}</li>)}
        </ul>
        <label className="model-status-search"><Search aria-hidden="true" /><span className="sr-only">{t("modelStatus.search")}</span><input type="search" value={query} placeholder={t("modelStatus.search")} onChange={event => setQuery(event.currentTarget.value)} /></label>
      </div>
      <div className="table-shell model-status-frame">
        <table className="model-status-table">
          <caption className="sr-only">{t("modelStatus.table")}</caption>
          <colgroup><col className="model-status-name-col" /><col className="model-status-history-col" />{columns.map(metric => <col key={metric.id} />)}</colgroup>
          <thead><tr><th scope="col">{t("common.model")}</th><th scope="col"><div className="model-status-axis"><span>{t(period === "day" ? "modelStatus.dayAgo" : "modelStatus.weekAgo")}</span><span>{t("modelStatus.now")}</span></div></th>{columns.map(metric => <th key={metric.id} scope="col">{t(metric.column)}</th>)}</tr></thead>
          <tbody>{rows.length ? rows.map(model => <tr key={model.id} className="model-status-row" data-state={model.metrics?.state ?? "unknown"}>
            <th scope="row"><strong className="model-status-name">{model.name}</strong></th>
            <td className="model-status-history-cell"><div className="model-status-axis model-status-mobile-axis" aria-hidden="true"><span>{t(period === "day" ? "modelStatus.dayAgo" : "modelStatus.weekAgo")}</span><span>{t("modelStatus.now")}</span></div><StatusHistory key={period} modelName={model.name} intervals={model.intervals} period={period} /></td>
            {columns.map(metric => <td className="model-status-value" key={metric.id} data-metric={metric.id}><span className="model-status-cell-label" aria-hidden="true">{t(metric.column)}</span><StatusValue metric={metric.id} values={model.metrics} /></td>)}
          </tr>) : <tr className="model-status-empty"><td colSpan={6}><span role="status">{t(all.length ? "modelStatus.noMatches" : "modelStatus.noModels")}</span></td></tr>}</tbody>
        </table>
      </div>
    </section></>}
  </section>;
}
