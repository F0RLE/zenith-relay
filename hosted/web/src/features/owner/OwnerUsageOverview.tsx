import { useCallback, useState } from "react";
import { readOwnerUsageOverview, type OwnerOverviewFilter } from "../../api/ownerOverview";
import { ownerUsageHref } from "../../api/ownerUsage";
import { ChevronDown, RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { UsageCharts } from "../overview/UsageCharts";
import { localInterval, ReportPeriods, ReportStatus, useReportRead } from "../usage/ReportTools";

export function OwnerUsageOverview() {
  const { t, locale } = useLocale();
  const [filter, setFilter] = useState<OwnerOverviewFilter>({ hours: 24 });
  const load = useCallback((signal: AbortSignal) => readOwnerUsageOverview(filter, signal), [filter]);
  const { data, error, lastData } = useReportRead(load, "/admin/login");
  const models = lastData?.models ?? [];
  const loading = !error && !data;
  const refresh = () => setFilter(previous => ({ ...previous }));
  const chooseModel = (model: string) => setFilter({ hours: filter.hours, ...(model ? { model } : {}) });
  const asOf = data && new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" }).format(data.to) + " UTC";
  return <section className="hosted-overview report-overview owner-overview" aria-label={t("nav.usage")}>
    <header className="report-overview-heading owner-overview-toolbar">
      <ReportPeriods hours={filter.hours} allTime onChange={hours => setFilter(previous => ({ ...previous, hours }))} />
      {(filter.model || models.length > 1) && <label className="catalog-select report-model">
        <span className="sr-only">{t("overview.statisticsModel")}</span>
        <select value={filter.model ?? ""} onChange={e => chooseModel(e.currentTarget.value)}>
          <option value="">{t("usage.allModels")}</option>
          {filter.model && !models.includes(filter.model) && <option value={filter.model}>{filter.model}</option>}
          {models.map(model => <option key={model} value={model}>{model}</option>)}
        </select>
        <ChevronDown aria-hidden="true" />
      </label>}
      <div className="owner-snapshot">
        {data && asOf && <time dateTime={new Date(data.to).toISOString()} title={localInterval(data.from, data.to, locale, true)}>
          {t("report.asOf", { time: asOf })}</time>}
      </div>
      <button type="button" className="icon-button report-refresh" aria-label={t("usage.refreshUsage")} title={t("usage.refreshUsage")}
        aria-disabled={loading} onClick={() => { if (!loading) refresh(); }}><RefreshCw aria-hidden="true" /></button>
    </header>
    {data?.moreModels && <p className="owner-model-limit">{t("report.modelListLimited", { count: models.length })}</p>}
    {data ? data.totals.requests > 0n ? <UsageCharts data={data} chargeLabel={t("overview.apiCharges")}
      intervalLink={index => ownerUsageHref({ errors: false, window: data.buckets[index]!, model: filter.model })} />
      : <div className="report-state"><p>{t("usage.noRequestsFound")}</p>
        {filter.model && <button type="button" className="button" onClick={() => chooseModel("")}>{t("usage.resetFilters")}</button>}</div>
      : error ? <ReportStatus error={error} retry={refresh} />
      : <div className="report-state" role="status"><p>{t("unavailable.connecting")}</p></div>}
  </section>;
}
