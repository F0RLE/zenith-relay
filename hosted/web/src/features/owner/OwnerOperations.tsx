import { useCallback, useState } from "react";
import { readOperationsEvents, readOperationsStatus } from "../../api/ownerOperations";
import type { readOwnerOverview } from "../../api/ownerSession";
import { RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { ReportStatus, useReportRead } from "../usage/ReportTools";
import { OwnerDiagnosticCapture } from "./OwnerDiagnosticCapture";
import { utcStamp } from "./ownerUsageDisplay";
import "./owner-operations.css";

const jobLabel = {
  request_reconciliation: "operations.requestReconciliation",
  usage_report_cleanup: "operations.usageCleanup",
  service_notice_cleanup: "operations.noticeCleanup",
  checkout_quote_cleanup: "operations.quoteCleanup",
  checkout_resume_cleanup: "operations.resumeCleanup",
  request_diagnostic_cleanup: "operations.diagnosticCleanup",
} as const;
const stateLabel = { succeeded: "operations.succeeded", failed: "operations.failed",
  stale: "operations.stale", unobserved: "operations.unobserved" } as const;

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;

export function OwnerOperations({ session }: { session: Session }) {
  const { t, locale } = useLocale();
  const [revision, setRevision] = useState(0);
  const [pages, setPages] = useState<string[]>([]);
  const before = pages.at(-1);
  const statusRead = useCallback((signal: AbortSignal) => readOperationsStatus(signal), [revision]);
  const eventsRead = useCallback((signal: AbortSignal) => readOperationsEvents(before, signal), [before, revision]);
  const status = useReportRead(statusRead, "/admin/login");
  const events = useReportRead(eventsRead, "/admin/login");
  const loadingEvents = !events.data && !events.error;
  const loading = (!status.data && !status.error) || loadingEvents;
  function refresh() {
    setPages([]);
    setRevision(n => n + 1);
  }
  const stamp = (at: number) => <time dateTime={new Date(at).toISOString()}>{utcStamp(at, locale)}</time>;
  return <div className="owner-operations">
    <div className="operations-bar">
      <div aria-live="polite">{status.data && <time dateTime={new Date(status.data.asOf).toISOString()}>
        {t("report.asOf", { time: utcStamp(status.data.asOf, locale) })}</time>}</div>
      <button type="button" className="icon-button report-refresh" aria-label={t("operations.refresh")}
        title={t("operations.refresh")} aria-disabled={loading} onClick={() => { if (!loading) refresh(); }}>
        <RefreshCw aria-hidden="true" />
      </button>
    </div>
    <div className="operations-columns">
      <section aria-labelledby="operations-jobs">
        <h2 id="operations-jobs">{t("operations.jobs")}</h2>
        {status.data ? <ul className="operation-list">
          {status.data.jobs.map(job => <li key={job.job} className="operation-row">
            <strong>{t(jobLabel[job.job])}</strong>
            <span className="operation-state" data-state={job.state}>{t(stateLabel[job.state])}</span>
            {job.observedAt !== null && <div className="operation-meta">{stamp(job.observedAt)}</div>}
          </li>)}
        </ul> : <div className="operations-empty"><ReportStatus error={status.error} /></div>}
      </section>
      <section aria-labelledby="operations-events">
        <h2 id="operations-events">{t("operations.history")}</h2>
        {events.data ? <>
          {events.data.partial && <p className="operation-coverage">{t("operations.partial")}</p>}
          {events.data.items.length ? <ul className="operation-list">
            {events.data.items.map(item => <li key={item.id} className="operation-row">
              <strong>{t(jobLabel[item.job])}</strong>
              <span className="operation-state" data-state={item.code === "job_failed" ? "failed" : "succeeded"}>
                {t(item.code === "job_failed" ? "operations.failed" : "operations.recovered")}
              </span>
              <div className="operation-meta">{stamp(item.last)}
                {item.count > 1 && <span>{t("operations.recordedFailures", { count: item.count.toLocaleString(locale) })}</span>}
              </div>
            </li>)}
          </ul> : <div className="operations-empty"><p>{t("operations.empty")}</p></div>}
        </> : <div className="operations-empty">{events.error === "cursor_unavailable"
          ? <p role="alert">{t("operations.cursorUnavailable")}</p> : <ReportStatus error={events.error} />}</div>}
        {(before || events.lastData?.next) && <nav className="report-pager operations-pager" aria-label={t("operations.history")}>
          <button type="button" className="button" aria-disabled={loadingEvents || !before} onClick={() => {
            if (!loadingEvents && before) setPages(p => p.slice(0, -1));
          }}>{t("pagination.previousPage")}</button>
          <button type="button" className="button" aria-disabled={!events.data?.next} onClick={() => {
            const next = events.data?.next;
            if (next) setPages(p => [...p, next]);
          }}>{t("pagination.nextPage")}</button>
        </nav>}
      </section>
    </div>
    <OwnerDiagnosticCapture session={session} revision={revision} />
  </div>;
}
