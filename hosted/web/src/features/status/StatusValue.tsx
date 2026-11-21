import { statusStates, type StatusMetrics } from "../../api/modelStatus";
import { useLocale } from "../../i18n/LocaleProvider";

export type StatusMetric = "availability" | "requests" | "latency" | "cache";

export function StatusValue({ metric, values, interval = false }: { metric: StatusMetric; values: StatusMetrics | undefined; interval?: boolean }) {
  const { t, locale } = useLocale();
  const percent = (n: number) => n.toLocaleString(locale, { style: "percent", maximumFractionDigits: 2 });
  let value: string | undefined;
  let detail: string | undefined;
  if (values) {
    const { state, monitoredMs, operationalMs, spanMs, requestMs, requests, cacheBps, cacheSamples, latencyMs, latencySamples } = values;
    const samples = (n: number, label: "modelStatus.samples" | "modelStatus.completionSamples") =>
      t(label, { known: n.toLocaleString(locale), total: (requests ?? 0).toLocaleString(locale) });
    switch (metric) {
      case "availability":
        if (monitoredMs > 0) {
          value = interval ? t(state === "unknown" ? "modelStatus.partial" : statusStates[state]) : percent(operationalMs / monitoredMs);
          if (monitoredMs < spanMs) detail = t("modelStatus.monitored", { percent: percent(monitoredMs / spanMs) });
        }
        break;
      case "requests":
        if (requests !== null) {
          value = requests.toLocaleString(locale);
          if (requestMs < spanMs) detail = t("modelStatus.recorded", { percent: percent(requestMs / spanMs) });
        }
        break;
      case "latency":
        if (latencyMs !== null) {
          value = latencyMs.toLocaleString(locale, { style: "unit", unit: "millisecond", unitDisplay: "short" });
          detail = samples(latencySamples, "modelStatus.completionSamples");
        }
        break;
      case "cache":
        if (cacheBps !== null) {
          value = percent(cacheBps / 10000);
          detail = samples(cacheSamples, "modelStatus.samples");
        }
        break;
    }
  }
  if (value === undefined) return <><span aria-hidden="true">—</span><span className="sr-only">{t("modelStatus.noData")}</span></>;
  const showDetail = interval || metric === "availability";
  return <><span title={!showDetail ? detail : undefined}>{value}</span>
    {detail && <small className={showDetail ? "model-status-coverage" : "sr-only"}>{detail}</small>}</>;
}
