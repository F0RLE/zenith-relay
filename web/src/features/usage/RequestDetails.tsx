import { CircleX, CircleCheck, Clock3, Download, KeyRound } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { executionLabels, generationSpeed } from "./model";
import type { RequestRecord } from "./model";
import { useLocale } from "../../i18n/LocaleProvider";
import { useFormatters } from "../../i18n/format";
import { RequestTechnicalDetails } from "./RequestTechnicalDetails";
import { RequestTokens } from "./RequestTokens";
import "./request-details.css";

function downloadDiagnostics(record: RequestRecord, summary: string) {
  // Only the customer's request summary. No Desktop IPC, credentials or raw bodies.
  const url = URL.createObjectURL(new Blob([summary], { type: "text/plain;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = `relay-request-${record.id.replace(/[^a-zA-Z0-9_-]/g, "_").slice(0, 100)}.txt`;
  link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function RequestDetails({ record, onClose }: { record: RequestRecord; onClose: () => void }) {
  const { t, localize } = useLocale();
  const { formatDuration, formatMoney, formatSpeed, formatTime, supportSummary } = useFormatters();
  const StatusIcon = record.execution === "completed" ? CircleCheck : record.execution === "failed" ? CircleX : Clock3;
  return <Dialog title={t("request.requestDetails")} onClose={onClose} className="hosted-request-details">
    <section className="request-overview" aria-label={t("request.requestResult")}>
      <div className="request-details-header">
        <div className="request-details-identity">
          <div className="request-details-model">
            <h3>{record.model.name}</h3>
            <span className="request-details-status" data-status={record.execution} role="img" aria-label={t(executionLabels[record.execution])} data-relay-tooltip={t(executionLabels[record.execution])} tabIndex={0}>
              <StatusIcon aria-hidden="true" />{record.execution !== "completed" && t(executionLabels[record.execution])}
            </span>
          </div>
          <dl className="request-details-subtitle">
            <div><dt><KeyRound aria-hidden="true" /><span className="sr-only">{t("common.apiKey")}</span></dt><dd data-relay-tooltip={record.key.mask} tabIndex={0}>{record.key.name}</dd></div>
            <div><dt><Clock3 aria-hidden="true" /><span className="sr-only">{t("usage.timeUtc")}</span></dt><dd><time dateTime={record.startedAt}>{formatTime(record.startedAt)} UTC</time></dd></div>
          </dl>
        </div>
        <button type="button" className="button request-details-export" aria-label={t("request.downloadRequestDiagnostics")} data-relay-tooltip={t("request.downloadRequestDetails")} onClick={() => downloadDiagnostics(record, supportSummary(record))}>
          <Download aria-hidden="true" />
        </button>
      </div>
      <dl className="request-details-metrics">
        <div className="request-charge-metric"><dt>{t("common.charged")}</dt><dd data-testid="request-charge-total">{formatMoney(record.billing.postedMicroUsd)}{record.billing.postedMicroUsd !== null && " $"}</dd></div>
        <div><dt>{t("request.firstOutput")}</dt><dd>{formatDuration(record.timing.firstTokenMs)}</dd></div>
        <div><dt>{t("request.responseTime")}</dt><dd>{formatDuration(record.timing.totalMs)}</dd></div>
        <div><dt>{t("usage.generationSpeed")}</dt><dd>{formatSpeed(generationSpeed(record))}</dd></div>
      </dl>
    </section>
    {record.execution === "failed" && <section className="request-notice" data-status="failed"><h3>{localize(record.response.result)}</h3>{record.response.guidance && <p>{localize(record.response.guidance)}</p>}</section>}
    {record.execution === "running" && <section className="request-notice"><h3>{t("request.requestInProgress")}</h3><p>{t("request.runningNotice")}</p></section>}
    {record.execution === "completed" && record.accounting === "pending" && <section className="request-notice"><h3>{t("request.responseCompleteAccountingPending")}</h3><p>{t("request.pendingNotice")}</p></section>}
    <RequestTokens record={record} />
    <RequestTechnicalDetails record={record} />
  </Dialog>;
}
