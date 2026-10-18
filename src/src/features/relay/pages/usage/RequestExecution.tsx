import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, CopyButton, StatusIcon } from "../../components/Ui";
import { loadRequestAttempts, type AttemptHistory, type AttemptPageLoader } from "./attemptHistory";
import { RequestSection } from "./RequestSection";
import type { UsageRow } from "./usageData";
import { formatDurationMs, formatErrorCategory, requestStatusLabel } from "./usageReportFormat";

type HistoryState =
  | { requestId: string | null; status: "loading" | "failed" }
  | { requestId: string | null; status: "loaded"; history: AttemptHistory };

/** Keeps the request's final result fixed while showing its sends in chronological order. */
export function RequestExecution({ row, loadPage }: { row: UsageRow; loadPage: AttemptPageLoader }) {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<HistoryState>({ requestId: row.requestId, status: "loading" });
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true;
    const requestId = row.requestId;
    setState({ requestId, status: "loading" });
    if (requestId) {
      void loadRequestAttempts(requestId, loadPage, () => active)
        .then((history) => { if (active) setState({ requestId, status: "loaded", history }); })
        .catch(() => { if (active) setState({ requestId, status: "failed" }); });
    }
    return () => { active = false; };
  }, [row.requestId, row.attempt, row.success, loadPage, revision]);
  if (!row.requestId) return null;
  const history = state.requestId === row.requestId && state.status === "loaded" ? state.history : null;
  // History and final rows use different storage IDs. Match the final send by its ordinal.
  const sends = [...history?.rows ?? []];
  const finalIndex = sends.findIndex((send) => send.attempt === row.attempt
    && send.candidateKey === row.candidateKey && send.success === row.success && send.httpStatus === row.httpStatus);
  if (finalIndex >= 0) sends[finalIndex] = row;
  else sends.push(row);
  sends.sort((left, right) => left.attempt - right.attempt || Date.parse(left.time) - Date.parse(right.time));
  const hasRepeats = sends.length > 1 || row.attempt > 1;
  const failed = state.requestId === row.requestId && state.status === "failed";
  const missing = history && (!history.complete || new Set(sends.map((send) => send.attempt)).size < Math.max(...sends.map((send) => send.attempt)));
  const decision = row.routing?.attemptOutcome;
  const stopReason = !row.success
    ? decision?.startsWith("retry_") ? "historyEndsBeforeResult"
      : decision === "tool_discovery" ? "historyEndsBeforeResult"
        : decision && decision !== "completed" ? `stops.${decision}` : hasRepeats ? "stopUnknown" : null
    : null;
  if (!hasRepeats && !failed && !missing) return null;
  const historyBadge = failed ? t("usage.execution.unavailable")
    : missing ? t("usage.execution.incomplete")
      : history ? t("usage.execution.sendCount", { count: sends.length }) : t("common.loading");
  return <RequestSection title={t("usage.execution.title")} badge={historyBadge} className="request-execution">
    {stopReason ? <p className="request-stop-reason">{t(`usage.execution.${stopReason}`)}</p> : null}
    {failed || missing ? <div className="request-history-notice" role="status">
      <span>{t(failed ? "usage.execution.loadFailed" : "usage.execution.partial")}</span>
      <Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={() => setRevision((value) => value + 1)}>{t("common.refresh")}</Button>
    </div> : hasRepeats && !history ? <p className="form-note" role="status">{t("usage.execution.loading")}</p> : null}
    {sends.length > 1 ? <ol className="request-execution-list">{sends.map((send) => {
      const message = send.upstreamError?.message;
      const lookup = send.routing?.attemptOutcome === "tool_discovery";
      const label = lookup ? t("usage.execution.decisions.tool_discovery") : requestStatusLabel(send, t);
      return <li key={`${send.attempt}:${send.id}`} aria-label={t("usage.execution.send", { number: send.attempt })}>
        <span className="request-send-marker"><StatusIcon status={send.success || lookup ? "ready" : "error"} label={label} /></span>
        <div className="request-send-content">
          <div className="request-send-heading">
            <strong><span className="request-send-index">{send.attempt}.</span> {send.connection}</strong>
            <span>{formatDurationMs(send.duration, i18n.resolvedLanguage ?? i18n.language, t)}{send.httpStatus != null ? ` · HTTP ${send.httpStatus}` : ""}</span>
          </div>
          <p className={send.success || lookup ? "request-send-success" : "request-send-error"}>
            {lookup ? label : send.success ? t("common.success") : formatErrorCategory(send.errorCategory, t)}
          </p>
          {send.routing?.attemptOutcome?.startsWith("retry_") ? <p className="request-send-decision">{t(`usage.execution.decisions.${send.routing.attemptOutcome}`)}</p> : null}
          {message && send !== row ? <details className="request-send-details">
            <summary>{t("usage.execution.providerDetails")}</summary>
            <div className="request-upstream-message"><pre>{message}</pre><CopyButton value={message} label={t("usage.copyUpstreamError")} /></div>
            {send.upstreamError?.requestId ? <code>{send.upstreamError.requestId}</code> : null}
          </details> : null}
        </div>
      </li>;
    })}</ol> : null}
  </RequestSection>;
}
