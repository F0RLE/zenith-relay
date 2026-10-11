import { Braces } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CopyButton } from "../../components/Ui";
import { CacheContextDetails } from "./CacheContextDetails";
import { RequestDisclosure } from "./RequestDisclosure";
import { RequestSection } from "./RequestSection";
import type { UsageRow } from "./usageData";
import { formatEndpointKind, formatErrorOrigin, formatObservedServiceTier, formatToolChoice, formatTerminalOutput } from "./usageReportFormat";

export function RequestTechnicalDetails({ row, debugEnabled }: { row: UsageRow; debugEnabled: boolean }) {
  const { t } = useTranslation();
  const routing = row.routing;
  const toolUse = row.toolUse;
  const hasTools = toolUse && (toolUse.clientToolCount > 0 || toolUse.forwardedToolCount > 0 || toolUse.toolCallCount > 0);
  const toolWarning = Boolean(
    row.success
      && toolUse
      && toolUse.forwardedToolCount > 0
      && toolUse.toolCallCount === 0
      && toolUse.terminalOutput === "text",
  );
  const observedTier = formatObservedServiceTier(row);
  const unresolvedRetry = !row.success && routing?.attemptOutcome?.startsWith("retry_");
  const separateEndpoint = routing?.endpointKind && routing.endpointKind !== row.wireApi;
  const hasErrorMetadata = !row.success && (row.httpStatus != null || row.errorOrigin || row.errorCategory
    || row.upstreamError?.httpStatus != null || row.upstreamError?.code || row.upstreamError?.errorType);
  if (!row.requestId && !row.upstreamError?.requestId && !hasErrorMetadata && !separateEndpoint && !observedTier && !routing && !hasTools) return null;
  return <RequestDisclosure title={t("usage.technicalDetails")} icon={<Braces />} className="request-technical-details">
    <div className="request-technical-grid">
      {row.requestId || row.upstreamError?.requestId ? <RequestSection title={t("usage.requestSections.identifiers")} className="request-identifiers-section">
        <dl className="request-details-list request-id-list">
          {row.requestId ? <div>
            <dt>{t("usage.localRequestId")}</dt>
            <dd className="request-details-copy-value"><code>{row.requestId}</code><CopyButton value={row.requestId} label={t("usage.copyRequestId")} /></dd>
          </div> : null}
          {row.upstreamError?.requestId ? <div>
            <dt>{t("usage.upstreamRequestId")}</dt>
            <dd className="request-details-copy-value"><code>{row.upstreamError.requestId}</code><CopyButton value={row.upstreamError.requestId} label={t("usage.copyUpstreamRequestId")} /></dd>
          </div> : null}
        </dl>
      </RequestSection> : null}
      {hasErrorMetadata || separateEndpoint || observedTier ? <RequestSection title={t(hasErrorMetadata ? "usage.requestSections.failure" : "usage.requestSections.response")} className="request-response-section">
        <dl className="request-details-list">
          {!row.success ? <>
            {row.httpStatus != null ? <div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus}</dd></div> : null}
            {row.errorOrigin ? <div><dt>{t("usage.errorOrigin")}</dt><dd>{formatErrorOrigin(row.errorOrigin, t)}</dd></div> : null}
            {row.errorCategory ? <div><dt>{t("usage.errorCategory")}</dt><dd><code>{row.errorCategory}</code></dd></div> : null}
            {row.upstreamError?.httpStatus != null && row.upstreamError.httpStatus !== row.httpStatus
              ? <div><dt>{t("usage.upstreamHttpStatus")}</dt><dd>{row.upstreamError.httpStatus}</dd></div> : null}
            {row.upstreamError?.code && row.upstreamError.code !== row.errorCategory
              ? <div><dt>{t("usage.upstreamErrorCode")}</dt><dd><code>{row.upstreamError.code}</code></dd></div> : null}
            {row.upstreamError?.errorType
              ? <div><dt>{t("usage.upstreamErrorType")}</dt><dd><code>{row.upstreamError.errorType}</code></dd></div> : null}
          </> : null}
          {separateEndpoint && routing?.endpointKind
            ? <div><dt>{t("usage.endpoint")}</dt><dd>{formatEndpointKind(routing.endpointKind, row.wireApi, t)}</dd></div> : null}
          {observedTier ? <div><dt>{t("usage.upstreamTier")}</dt><dd>{observedTier}</dd></div> : null}
        </dl>
      </RequestSection> : null}
      {routing ? <RequestSection title={t("usage.requestSections.route")} className="request-route-section">
        <dl className="request-details-list">
          <div className="request-route-reason"><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div>
          <div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div>
          {row.candidateKind === "account" && routing.quotaRemainingBasisPoints != null
            ? <div><dt>{t("usage.quotaAtSelection")}</dt><dd>{(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%</dd></div> : null}
          {debugEnabled ? <>
            <div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div>
            <div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div>
          </> : null}
        </dl>
        {unresolvedRetry ? <p className="form-note request-routing-note">{t("usage.execution.historyEndsBeforeResult")}</p> : null}
      </RequestSection> : null}
      {hasTools && toolUse ? <RequestSection title={t("usage.requestSections.tools")} className="request-tool-details">
        <dl className="request-details-list request-tools-list">
          <div><dt>{t("usage.clientTools")}</dt><dd>{toolUse.clientToolCount}</dd></div>
          {toolUse.forwardedToolCount !== toolUse.clientToolCount
            ? <div><dt>{t("usage.forwardedTools")}</dt><dd>{toolUse.forwardedToolCount}</dd></div>
            : null}
          <div><dt>{t("usage.toolCallsReturned")}</dt><dd>{toolUse.toolCallCount}</dd></div>
          {toolUse.toolChoice && toolUse.toolChoice !== "auto"
            ? <div><dt>{t("usage.toolChoice")}</dt><dd>{formatToolChoice(toolUse.toolChoice, t)}</dd></div>
            : null}
          <div><dt>{t("usage.terminalOutput")}</dt><dd>{formatTerminalOutput(toolUse.terminalOutput, t)}</dd></div>
        </dl>
        {toolWarning ? <p className="form-note warning-text">{t("usage.toolCallMissing", { count: toolUse.forwardedToolCount })}</p> : null}
      </RequestSection> : null}
    </div>
    {debugEnabled && routing?.cacheContext
      ? <RequestDisclosure title={t("usage.cacheContext.title")} className="request-context-details">
        <CacheContextDetails diagnostics={routing.cacheContext} />
      </RequestDisclosure> : null}
  </RequestDisclosure>;
}
