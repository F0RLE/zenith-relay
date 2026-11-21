import { useCallback, useEffect, useState } from "react";
import { readUsageOverview, usageLink } from "../../api/usageReport";
import { ChevronDown, RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { localInterval, reportView, ReportPeriods, ReportStatus, useReportRead, useReportSource } from "../usage/ReportTools";
import { UsageCharts } from "./UsageCharts";

export function CustomerOverview({ csrf }: { csrf: string }) {
  const { t, locale } = useLocale();
  const [view] = useState(() => reportView(true));
  const [model, setModel] = useState(view?.model ?? "");
  const source = useReportSource(view, csrf);
  const report = source.report;
  const load = useCallback((signal: AbortSignal) => readUsageOverview({ report: report!, model: model || undefined }, signal), [report, model]);
  const { data, error } = useReportRead(report ? load : null);
  const loadedHours = data?.hours;
  useEffect(() => {
    if (!report || !loadedHours) return;
    const q = new URLSearchParams({ report, hours: String(loadedHours), ...(model ? { model } : {}) });
    history.replaceState(null, "", location.pathname + "?" + q);
  }, [report, loadedHours, model]);
  const interval = data && localInterval(data.from, data.to, locale, true);
  const problem = view ? source.error ?? error : "invalid_filter";
  function refresh(hours = data?.hours ?? source.hours) {
    const q = new URLSearchParams({ hours: String(hours), ...(model ? { model } : {}) });
    history.replaceState(null, "", location.pathname + "?" + q);
    source.refresh(hours);
  }
  const retry = view ? () => refresh() : () => location.assign("/app/overview");
  return <section className="hosted-overview report-overview" aria-labelledby="customer-overview-title">
    <header className="page-heading overview-heading report-overview-heading">
      <h1 id="customer-overview-title" tabIndex={interval ? 0 : undefined} data-relay-tooltip={interval}>{t("nav.overview")}</h1>
      <div className="overview-controls report-overview-controls">
        {(model || (data?.models.length ?? 0) > 1) && <label className="catalog-select report-model">
          <span className="sr-only">{t("overview.statisticsModel")}</span>
          <select value={model} onChange={e => setModel(e.currentTarget.value)}>
            <option value="">{t("usage.allModels")}</option>
            {model && !data?.models.includes(model) && <option value={model}>{model}</option>}
            {data?.models.map(m => <option key={m} value={m}>{m}</option>)}
          </select>
          <ChevronDown aria-hidden="true" />
        </label>}
        <ReportPeriods hours={data?.hours ?? source.hours} disabled={!view || (!report && !problem)} onChange={refresh} />
        <button type="button" className="icon-button report-refresh" aria-label={t("usage.refreshUsage")} title={t("usage.refreshUsage")}
          onClick={retry} disabled={!problem && !data}><RefreshCw aria-hidden="true" /></button>
      </div>
    </header>
    {data && report ? data.totals.requests > 0
      ? <UsageCharts data={data} intervalLink={bucket => usageLink({ report, model: model || undefined, bucket }, data.hours)} />
      : <div className="report-state"><p>{t("usage.noRequestsFound")}</p>
        {model && <button type="button" className="button" onClick={() => setModel("")}>{t("usage.resetFilters")}</button>}</div>
      : <ReportStatus error={problem} />}
  </section>;
}
