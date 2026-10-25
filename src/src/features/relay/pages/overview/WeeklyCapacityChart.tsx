import { memo, useEffect, useMemo, useState } from "react";
import { ChartNoAxesCombined } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { WeeklyCapacityReport } from "../../api/types/weeklyCapacity";
import { OptionMenu } from "../../components/Ui";
import { formatApproximateUsd, formatScaledUsd } from "../../currencyFormatting";
import { OverviewChart } from "./OverviewAnalytics";
import { DAY_MS, HOUR_MS, type WindowBucket } from "./overviewAnalyticsModel";
import { cachedCapacity, capacityCurrent, capacityGroupOptions, loadCapacity, nextCapacityExpiry } from "./weeklyCapacityModel";

type Props = { windows: WindowBucket[]; revision: number; accountId?: string };

function WeeklyCapacityChart({ windows, revision, accountId }: Props) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const fromMs = windows[0]?.startMs ?? 0;
  const toMs = windows.at(-1)?.endMs ?? fromMs;
  const bucketMs = windows.length === 24 ? HOUR_MS : DAY_MS;
  const key = JSON.stringify([fromMs, toMs, bucketMs, accountId ?? ""]);
  const [loaded, setLoaded] = useState<{ key: string; revision: number; report: WeeklyCapacityReport | null }>(() => ({ key, revision, report: cachedCapacity(key, revision) }));
  const [error, setError] = useState(false);
  const [now, setNow] = useState(Date.now);
  const [planChoice, setPlanChoice] = useState("");
  const [groupChoice, setGroupChoice] = useState("");
  const report = loaded.key === key && loaded.revision === revision ? loaded.report : cachedCapacity(key, revision);
  useEffect(() => {
    let active = true;
    setError(false);
    void loadCapacity(key, revision, () => relayCommands.localWeeklyCapacity({ fromMs, toMs, bucketMs, ...(accountId ? { accountId } : {}) }))
      .then((report) => { if (active) setLoaded({ key, revision, report }); })
      .catch(() => { if (active) setError(true); });
    return () => { active = false; };
  }, [key, revision, fromMs, toMs, bucketMs, accountId, now]);
  useEffect(() => {
    if (!report) return;
    const at = nextCapacityExpiry(report, Date.now());
    if (at == null) return;
    // One local expiry wakeup, not provider polling or a renderer interval.
    const timer = window.setTimeout(() => setNow(Date.now()), Math.max(1, at - Date.now() + 1));
    return () => window.clearTimeout(timer);
  }, [report, now]);
  const plans = useMemo(() => [...new Set([...(report?.summaries.map((summary) => summary.plan) ?? []), ...(report?.groups.map((group) => group.plan) ?? []), ...(report?.events.map((event) => event.plan) ?? [])])], [report]);
  const plan = plans.includes(planChoice) ? planChoice : plans[0] ?? "";
  const groups = useMemo(() => report?.groups.filter((group) => group.plan === plan) ?? [], [report, plan]);
  const groupOptions = useMemo(() => capacityGroupOptions(groups), [groups]);
  const selectedGroup = groups.find((group) => group.id === groupChoice);
  const series = selectedGroup ?? report?.summaries.find((summary) => summary.plan === plan);
  const points = useMemo(() => {
    const byTime = new Map(series?.points.map((point) => [point.atMs, point]));
    return windows.map((window) => byTime.get(window.startMs) ?? null);
  }, [series, windows]);
  const current = capacityCurrent(series, Date.now());
  const values = points.map((point) => point ? point.medianMicroUsd / 1_000_000 : null);
  const bands = points.map((point) => point ? { low: point.lowMicroUsd / 1_000_000, high: point.highMicroUsd / 1_000_000 } : null);
  const markers = windows.map((window) => {
    const events = report?.events.filter((event) => event.plan === plan && event.atMs === window.startMs) ?? [];
    return events.length ? events.map((event) => t(`overview.capacity.events.${event.kind}`, { count: event.count })).join(" · ") : null;
  });
  const money = (value: number) => formatApproximateUsd(value, locale);
  const date = (at: number) => new Date(at).toLocaleString(locale, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  return <OverviewChart
    icon={<ChartNoAxesCombined aria-hidden />}
    title={t("overview.capacity.title")}
    summary={current ? money(current.point.medianMicroUsd / 1_000_000) : "—"}
    summaryHint={current ? t(current.stale ? "overview.capacity.stale" : "overview.capacity.current", { date: date(current.point.observedAtMs) }) : t("overview.capacity.waiting")}
    seriesValues={values} windows={windows} variant="line" tone="weekly-capacity"
    formatValue={money} formatAxis={(value) => formatScaledUsd(value, locale)} bandValues={bands} markers={markers}
    emptyLabel={error ? t("overview.capacity.unavailable") : !report ? t("common.loading") : t("overview.noMeasurements")}
    controls={<div className="overview-capacity-controls">
      {plans.length > 1 ? <OptionMenu label={t("overview.capacity.plan")} value={plan} onChange={(value) => { setPlanChoice(value); setGroupChoice(""); }} options={plans.map((value) => ({ value, label: value }))} fitContent align="start" />
        : plan ? <span className="overview-capacity-plan" aria-label={`${t("overview.capacity.plan")}: ${plan}`}>{plan}</span> : null}
      {groups.length > 1 ? <OptionMenu label={t("overview.capacity.workload")} value={selectedGroup?.id ?? ""} onChange={setGroupChoice} options={[{ value: "", label: t("overview.capacity.allModels") }, ...groupOptions]} fitContent align="start" />
        : plan ? <span className="overview-capacity-plan">{t("overview.capacity.allModels")}</span> : null}
    </div>}
    details={(index) => {
      const point = points[index];
      return point && point.accountCount > 1 && point.lowMicroUsd < point.highMicroUsd
        ? <small>{t("overview.capacity.spread", { low: money(point.lowMicroUsd / 1_000_000), high: money(point.highMicroUsd / 1_000_000) })}</small>
        : null;
    }}
  />;
}

export default memo(WeeklyCapacityChart);
