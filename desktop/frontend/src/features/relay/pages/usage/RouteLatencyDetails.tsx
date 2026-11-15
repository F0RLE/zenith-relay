import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { RoutingDiagnostics } from "../../api/types";
import { formatDurationMs } from "./usageReportFormat";

export function RouteLatencyDetails({ diagnostics }: { diagnostics: RoutingDiagnostics["routeLatency"] }) {
  const { t, i18n } = useTranslation();
  if (!diagnostics) return null;
  const duration = (value: number | null) => value == null ? "—" : formatDurationMs(value, i18n.language, t);
  const field = (key: string, value: ReactNode) => <div key={key}><dt>{t(key)}</dt><dd>{value}</dd></div>;
  const age = diagnostics.lastObservedAtMs == null ? null
    : Math.max(0, diagnostics.asOfMs - diagnostics.lastObservedAtMs);
  return <>
    {field("usage.routeLatency.title", <span
      tabIndex={0}
      data-relay-tooltip={t("usage.routeLatency.scope")}
    >
      {diagnostics.state === "ready"
        ? `P50 ${duration(diagnostics.p50Ms)} · P90 ${duration(diagnostics.p90Ms)} · P95 ${duration(diagnostics.p95Ms)}`
        : t(`usage.routeLatency.states.${diagnostics.state}`, { minimum: diagnostics.minimumSamples })}
    </span>)}
    {field("usage.routeLatency.samples", t("usage.routeLatency.sampleSummary", {
      measured: diagnostics.sampleCount, unknown: diagnostics.unknownCount, window: duration(diagnostics.windowMs),
    }))}
    {age != null ? field("usage.routeLatency.freshness", duration(age)) : null}
  </>;
}
