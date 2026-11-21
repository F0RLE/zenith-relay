import { CircleAlert, Clock3, Download, Link2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, CopyButton, Dialog } from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { useRelayState } from "../../state/RelayStateProvider";
import { tokenSpeed } from "../../usageSpeed";
import { formatFullNumber, measuredCacheHitRate } from "../../usageTotals";
import { RequestReceiptDetails } from "./RequestReceiptDetails";
import { RequestTokenLedger } from "./RequestTokenLedger";
import { usageSpeedSample, type UsageRow } from "./usageData";
import { formatDurationMs, formatErrorCategory, missingRequiredToolCall, prefixErrorOrigin, requestStatusHint, requestStatusKind, requestStatusLabel } from "./usageReportFormat";
import { RequestDetailMetric, RequestStatusGlyph, SpeedValue } from "./usageReportParts";

export function RequestDetails({ row, local, onClose }: {
  row: UsageRow; local: boolean; onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const { perform, busy } = useRelayState();
  const status = requestStatusKind(row);
  const timestamp = Date.parse(row.time);
  const speed = tokenSpeed(usageSpeedSample(row));
  const cacheShare = measuredCacheHitRate(row.inputTokens, row.cachedInputTokens);
  const error = row.upstreamError;
  const message = error?.message?.trim();
  const providerMessage = message ? prefixErrorOrigin(row.errorOrigin, message) : null;
  const stop = !row.success ? row.routing?.attemptOutcome : null;
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} className="request-details-dialog"
    backdropClassName="request-details-backdrop" restoreFocus="after-paint">
    <header className="request-details-header">
      <div className="request-details-identity">
        <div className="request-details-model">
          <h3>{row.model ?? t("common.unknown")}</h3>
          <span className="request-details-status" data-status={status}
            role={status === "success" ? "img" : undefined}
            aria-label={status === "success" ? requestStatusHint(row, t) : undefined}
            data-relay-tooltip={requestStatusHint(row, t)}>
            <RequestStatusGlyph row={row} />
            {status !== "success" ? requestStatusLabel(row, t) : null}
          </span>
        </div>
        {local && row.requestId ? <Button className="request-details-export" icon={<Download size={15} aria-hidden />}
          aria-label={t("usage.exportDiagnostics")} title={t("usage.exportDiagnosticsHint")}
          busy={busy === "request-diagnostics"}
          onClick={() => void perform("request-diagnostics", () => relayCommands.exportRequestDiagnostics(row.requestId!), undefined, { skipRefresh: true })}>
          <span className="request-details-export-label">{t("usage.exportDiagnosticsShort")}</span>
        </Button> : null}
      </div>
      <dl className="request-details-subtitle">
        <div><dt><Link2 aria-hidden /><span className="sr-only">{t("usage.requestConnection")}</span></dt><dd>{row.connection}</dd></div>
        {Number.isFinite(timestamp) ? <div><dt><Clock3 aria-hidden /><span className="sr-only">{t("usage.time")}</span></dt>
          <dd><time dateTime={row.time}>{new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "medium" }).format(timestamp)}</time></dd></div> : null}
      </dl>
    </header>
    {status === "error" ? <section className="request-details-error" aria-label={t("usage.errorDetails")}>
      <div className="request-outcome-heading">
        <CircleAlert aria-hidden />
        <h3>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : t("common.failed")}</h3>
        {row.httpStatus != null ? <code>HTTP {row.httpStatus}</code> : null}
      </div>
      {stop === "execution_uncertain" || stop === "client_disconnected" ? <p className="request-outcome-note">{t(`usage.execution.stops.${stop}`)}</p> : null}
      {providerMessage ? <div className="request-provider-response">
        <div className="request-upstream-message"><pre>{providerMessage}</pre><CopyButton value={providerMessage} label={t("usage.copyUpstreamError")} /></div>
        {error?.truncated ? <p className="form-note">{t("usage.upstreamErrorTruncated")}</p> : null}
      </div> : null}
    </section> : null}
    {row.success && missingRequiredToolCall(row.toolUse) ? <p className="request-tool-warning">
      <CircleAlert size={16} aria-hidden />{t("usage.toolCallMissing")}
    </p> : null}
    <div className="request-details-content">
      <dl className="request-details-metrics" aria-label={t("usage.requestSections.metrics")}>
        <RequestDetailMetric label={t("usage.responseTime")} value={formatDurationMs(row.duration, locale, t)} />
        {row.ttft != null && !row.imageUsage ? <RequestDetailMetric label={t("usage.firstResponse")} value={formatDurationMs(row.ttft, locale, t)} /> : null}
        {speed != null && !row.imageUsage ? <RequestDetailMetric label={t("usage.generationSpeed")} value={<SpeedValue value={speed} locale={locale} unit={t("usage.tokensPerSecondUnit")} />} /> : null}
        {row.imageUsage ? <RequestDetailMetric label={t("usage.imagesCompleted")} value={formatFullNumber(row.imageUsage.generatedImages, locale)} /> : null}
        {cacheShare != null ? <RequestDetailMetric label={t("usage.cachedShare")}
          value={formatNumber(cacheShare / 100, locale, { style: "percent", maximumFractionDigits: 1 })} /> : null}
      </dl>
      <RequestTokenLedger row={row} />
    </div>
    <RequestReceiptDetails row={row} />
  </Dialog>;
}
