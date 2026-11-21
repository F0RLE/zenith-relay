import { useCallback, useEffect, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { isExecution, isHours, isModel, startUsageReport, type Hours, type UsageSelection } from "../../api/usageReport";
import { useLocale } from "../../i18n/LocaleProvider";

const windows = [[24, "period.24Hours"], [168, "period.7Days"], [720, "period.30Days"]] as const;
type View = UsageSelection & { hours: Hours; pinned: string | undefined; id: string | undefined };

export function reportView(overview: boolean, query = location.search): View | null {
  const q = new URLSearchParams(query);
  const hours = q.has("hours") ? Number(q.get("hours")) : 24;
  const pinned = q.get("report") ?? undefined, model = q.get("model") ?? undefined;
  const raw = q.get("bucket"), bucket = raw === null ? undefined : /^\d{1,2}$/.test(raw) ? Number(raw) : -1;
  const id = q.get("id") ?? undefined;
  const keyId = q.get("key_id") ?? undefined, execution = q.get("execution") ?? undefined;
  if (["hours", "report", "model", "bucket", "id", "key_id", "execution"].some(k => q.getAll(k).length > 1)
    || !isHours(hours) || (pinned !== undefined && !uuidText.test(pinned))
    || (model !== undefined && !isModel(model))
    || (keyId !== undefined && (overview || !uuidText.test(keyId)))
    || (execution !== undefined && (overview || !isExecution(execution)))
    || (bucket !== undefined && (!pinned || bucket < 0 || bucket >= 30 || overview))
    || (id !== undefined && (overview || !uuidText.test(id) || (!pinned && (model || keyId || execution))))) return null;
  return { hours, pinned, model, bucket, keyId, execution, id };
}

type ReportRead<T> = { data?: T; error?: string; lastData?: T | undefined };
export function useReportRead<T>(load: ((signal: AbortSignal) => Promise<T>) | null, loginPath = "/login"): ReportRead<T> {
  const [state, setState] = useState<ReportRead<T> & { load: typeof load }>();
  useEffect(() => {
    if (!load) return;
    const c = new AbortController();
    const timer = setTimeout(() => c.abort("timeout"), 12000);
    let active = true;
    void load(c.signal).then(data => { if (active) setState({ load, data, lastData: data }); }).catch(e => {
      if (!active) return;
      if (e instanceof ControlError && e.status === 401) location.replace(loginPath);
      setState(previous => ({ load, lastData: previous?.lastData,
        error: e instanceof ControlError ? e.code ?? "unavailable" : "unavailable" }));
    }).finally(() => clearTimeout(timer));
    return () => { active = false; clearTimeout(timer); c.abort(); };
  }, [load, loginPath]);
  return load && state?.load === load ? state : { lastData: state?.lastData };
}

export function useReportSource(view: View | null, csrf: string) {
  const [source, setSource] = useState<{ hours: Hours; pinned: string | undefined }>({ hours: view?.hours ?? 24, pinned: view?.pinned });
  const start = useCallback((c: AbortSignal) => startUsageReport(source.hours, csrf, c), [source, csrf]);
  const created = useReportRead(view && !view.id && !source.pinned ? start : null);
  function refresh(hours = source.hours) {
    setSource({ hours, pinned: undefined });
  }
  return { report: source.pinned ?? created.data, pinned: source.pinned, hours: source.hours, error: created.error, refresh };
}

type PeriodSelection = { hours: Hours | undefined; allTime?: false; onChange: (h: Hours) => void }
  | { hours: Hours | 0; allTime: true; onChange: (h: Hours | 0) => void };
export function ReportPeriods({ hours, onChange, disabled, allTime }: PeriodSelection & { disabled?: boolean }) {
  const { t } = useLocale();
  return <div className="history-period" role="group" aria-label={t("overview.statisticsPeriod")}>
    {windows.map(([v, label]) => <button type="button" className="button" key={v} aria-pressed={v === hours}
      disabled={disabled} onClick={() => { if (hours !== v) onChange(v); }}>{t(label)}</button>)}
    {allTime && <button type="button" className="button" aria-pressed={hours === 0} disabled={disabled}
      onClick={() => { if (hours !== 0) onChange(0); }}>{t("period.allTime")}</button>}
  </div>;
}

export function reportErrorLabel(error: string, exporting = false) {
  return error === "report_not_found" ? "report.expired" : error === "report_busy" ? "report.busy"
    : error === "export_busy" ? "report.exportBusy"
    : error === "report_too_large" ? "report.tooLarge" : error === "invalid_filter" ? "report.invalid"
    : exporting ? "report.downloadFailed" : "unavailable.server";
}

export function ReportStatus({ error, retry }: { error?: string | undefined; retry?: () => void }) {
  const { t } = useLocale();
  if (!error) return <p role="status">{t("unavailable.connecting")}</p>;
  return <div className="report-state" role="alert"><p>{t(reportErrorLabel(error))}</p>
    {retry && error !== "report_too_large" && <button className="button" type="button" onClick={retry}>{t("common.retry")}</button>}</div>;
}

export function localInterval(from: number, to: number, locale: string, exact = false) {
  const format = new Intl.DateTimeFormat(locale, { timeZone: "UTC", year: "numeric", month: "short", day: "numeric",
    hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23", ...(exact ? { fractionalSecondDigits: 3 } as const : {}) });
  return format.formatRange(from, to) + " UTC";
}
