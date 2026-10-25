import { Braces } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CopyButton } from "../../components/Ui";
import { CacheContextDetails } from "./CacheContextDetails";
import { RequestDisclosure } from "./RequestDisclosure";
import { RequestSection } from "./RequestSection";
import type { UsageRow } from "./usageData";
import { formatContextBytes, formatDurationMs, formatEndpointKind, formatErrorOrigin, formatModelId, formatObservedServiceTier, formatToolChoice, formatTerminalOutput, formatTransport, missingRequiredToolCall, requestStatusKind } from "./usageReportFormat";

export function RequestTechnicalDetails({ row, debugEnabled }: { row: UsageRow; debugEnabled: boolean }) {
  const { t, i18n } = useTranslation();
  const routing = row.routing;
  const transport = routing?.transport;
  const payload = transport?.requestPayload;
  // Schema validation is included in bodyMs, so preparation parts are not additive.
  const preparationTimings = [
    ["preparationBodyTime", transport?.requestPreparation?.bodyMs],
    ["preparationSchemaTime", transport?.requestPreparation?.schemaValidationMs],
    ["preparationCatalogTime", transport?.requestPreparation?.catalogMs],
    ["preparationAttachmentsTime", transport?.requestPreparation?.attachmentsMs],
  ] as const;
  // These are dispatch-relative offsets, not durations to add to preparation.
  const responseTimings = ([
    ["responseHeadersTime", transport?.responseHeadersMs],
    ["clientFirstOutputTime", transport?.clientFirstOutputMs],
    ["upstreamResponseTime", transport?.upstreamResponseMs],
    ["clientResponseReadyTime", transport?.clientResponseReadyMs],
  ] as const)
    // Keep measured zero; buffered replies can emit their first output after collection.
    .flatMap(([label, milliseconds]) => milliseconds == null ? [] : [[label, milliseconds] as const])
    .sort(([, left], [, right]) => left - right);
  const hasPreparationTimings = preparationTimings.some(([, milliseconds]) => milliseconds != null);
  const hasPreparation = transport?.requestPreparationMs != null || hasPreparationTimings;
  const hasTimings = hasPreparation || responseTimings.length > 0;
  const requestFailed = requestStatusKind(row) === "error";
  const toolUse = row.toolUse;
  const hasTools = toolUse && (toolUse.clientToolCount > 0 || toolUse.forwardedToolCount > 0 || toolUse.toolCallCount > 0
    || toolUse.requestedParallelToolCalls != null || toolUse.upstreamToolCallCount != null);
  const toolWarning = row.success && missingRequiredToolCall(toolUse);
  const clientContext = toolUse?.clientContext;
  // BPS embeds tool schemas in upstream instructions instead of a native tools field.
  const upstreamContext = toolUse?.upstreamContext;
  const hasContext = Boolean(clientContext || upstreamContext);
  const catalog = toolUse?.basisPointsCatalog;
  const contextBytes = (client: number | undefined, upstream: number | undefined) =>
    `${formatContextBytes(client, i18n.language)} → ${formatContextBytes(upstream, i18n.language)}`;
  const observedTier = formatObservedServiceTier(row);
  // This is the provider-reported model, not an independently verified identity.
  const upstreamModel = routing?.upstreamResponseModel;
  const modelAlias = row.requestedModel && formatModelId(row.requestedModel, row.routedModel) !== row.requestedModel
    ? row.requestedModel : null;
  const requestedModel = modelAlias ?? (upstreamModel ? row.requestedModel : null);
  const separateEndpoint = routing?.endpointKind && routing.endpointKind !== row.wireApi;
  const hasErrorMetadata = requestFailed && (row.httpStatus != null || row.errorOrigin || row.errorCategory
    || row.upstreamError?.httpStatus != null || row.upstreamError?.code || row.upstreamError?.errorType);
  const hasDiagnostics = debugEnabled && (routing || hasTools || hasContext);
  if (!row.requestId && !row.upstreamError?.requestId && !modelAlias && !upstreamModel && !hasErrorMetadata && !separateEndpoint && !observedTier && !hasDiagnostics && !toolWarning) return null;
  return <RequestDisclosure title={t("usage.technicalDetails")} icon={<Braces />} className="request-technical-details">
    {() => <>
    <div className="request-technical-grid">
      {hasErrorMetadata || separateEndpoint || observedTier ? <RequestSection title={t(hasErrorMetadata ? "usage.requestSections.failure" : "usage.requestSections.response")} className="request-response-section">
        <dl className="request-details-list request-technical-fields">
          {requestFailed ? <>
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
      {debugEnabled && routing ? <RequestSection title={t("usage.requestSections.route")} className="request-route-section">
        <dl className="request-details-list request-technical-fields">
          <div className="request-route-reason"><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div>
          <div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div>
          {transport ? <>
            <div><dt>{t("usage.transportPath")}</dt><dd>{formatTransport(row.transport, t)} → {formatTransport(transport.upstream, t)}</dd></div>
            <div><dt>{t("usage.transportMode")}</dt><dd>{t(`usage.transportModes.${transport.mode}`)}</dd></div>
            {transport.fallbackReason ? <div><dt>{t("usage.transportFallback")}</dt><dd>{t(`usage.transportFallbackReasons.${transport.fallbackReason}`)}</dd></div> : null}
            {transport.responseFormat ? <div><dt>{t("usage.upstreamResponseFormat")}</dt><dd>{t(`usage.transportResponseFormats.${transport.responseFormat}`)}</dd></div> : null}
            {transport.handshakeMs != null ? <div><dt>{t("usage.handshakeMs")}</dt><dd>{formatDurationMs(transport.handshakeMs, i18n.language, t)}</dd></div> : null}
            {transport.connectionReused != null ? <div><dt>{t("usage.connectionReuse")}</dt><dd>{t(transport.connectionReused ? "usage.connectionReused" : "usage.connectionOpened")}</dd></div> : null}
            {payload ? <>
              <div><dt>{t("usage.requestPayloadMode")}</dt><dd>{t(`usage.requestPayloadModes.${payload.mode}`)}</dd></div>
              {payload.fullRequestReason ? <div><dt>{t("usage.fullRequestReason")}</dt><dd>{t(`usage.fullRequestReasons.${payload.fullRequestReason}`)}</dd></div> : null}
              <div><dt>{t("usage.requestPayloadBytes")}</dt><dd>{contextBytes(payload.jsonBytes, payload.bodyBytes)}</dd></div>
              <div><dt>{t("usage.requestEncoding")}</dt><dd>{payload.contentEncoding === "zstd" ? "Zstd" : t("usage.requestUncompressed")}</dd></div>
              {payload.compressionMs != null ? <div><dt>{t("usage.requestCompressionTime")}</dt><dd>{formatDurationMs(payload.compressionMs, i18n.language, t)}</dd></div> : null}
              {payload.httpSendToHeadersMs != null ? <div><dt>{t("usage.requestHttpSendTime")}</dt><dd>{formatDurationMs(payload.httpSendToHeadersMs, i18n.language, t)}</dd></div> : null}
            </> : null}
          </> : null}
          {row.candidateKind === "account" && routing.quotaRemainingBasisPoints != null
            ? <div><dt>{t("usage.quotaAtSelection")}</dt><dd>{(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%</dd></div> : null}
          <div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div>
          <div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div>
        </dl>
      </RequestSection> : null}
      {debugEnabled && hasTimings ? <RequestSection title={t("usage.requestSections.timings")} className="request-timings-section">
        {hasPreparation ? <div className="request-preparation-details">
          <dl className="request-details-list request-preparation-total">
            <div><dt>{t("usage.requestPreparationTime")}</dt><dd>{transport?.requestPreparationMs != null
              ? formatDurationMs(transport.requestPreparationMs, i18n.language, t) : "—"}</dd></div>
          </dl>
          {hasPreparationTimings ? <dl className="request-details-list request-technical-fields">
            {preparationTimings.map(([label, milliseconds]) => milliseconds != null
              ? <div key={label}><dt>{t(`usage.${label}`)}</dt><dd>{formatDurationMs(milliseconds, i18n.language, t)}</dd></div> : null)}
          </dl> : null}
        </div> : null}
        {responseTimings.length > 0 ? <div className="request-response-timings">
          <h4>{t("usage.requestSections.afterDispatch")}</h4>
          <dl className="request-details-list request-timing-metrics">
            {responseTimings.map(([label, milliseconds]) => <div key={label}>
              <dt>{t(`usage.${label}`)}</dt><dd>+{formatDurationMs(milliseconds, i18n.language, t)}</dd>
            </div>)}
          </dl>
        </div> : null}
      </RequestSection> : null}
      {debugEnabled && hasContext ? <RequestSection title={t("usage.contextSize.title")} className="request-context-section">
        <div className="request-context-columns">
          <dl className="request-details-list">
            {(["bodyBytes", "instructionBytes", "conversationBytes", "toolCatalogBytes"] as const).map((field, index) => <div key={field}>
              <dt>{t(`usage.contextSize.fields.${index}`)}</dt><dd>{contextBytes(clientContext?.[field], upstreamContext?.[field])}</dd>
            </div>)}
          </dl>
          {catalog ? <dl className="request-details-list">
            <div><dt>{t("usage.contextSize.catalogMode")}</dt><dd>{t(`usage.contextSize.modes.${catalog.mode}`)}</dd></div>
            <div><dt>{t("usage.contextSize.schemas")}</dt><dd>{contextBytes(catalog.fullSchemaBytes, catalog.renderedSchemaBytes)}</dd></div>
            <div><dt>{t("usage.contextSize.loaded")}</dt><dd>{catalog.loadedToolCount} / {toolUse?.forwardedToolCount ?? 0}</dd></div>
            {catalog.mode === "discovery" ? <>
              <div><dt>{t("usage.contextSize.lookups")}</dt><dd>{catalog.discoveryRounds}</dd></div>
              {catalog.discoveryLatencyMs != null ? <div><dt>{t("usage.contextSize.lookupDuration")}</dt><dd>{formatDurationMs(catalog.discoveryLatencyMs, i18n.language, t)}</dd></div> : null}
              {catalog.discoveryRounds > 0 ? <div><dt>{t("usage.contextSize.lookupTokens")}</dt><dd>{catalog.discoveryInputTokens ?? "—"} / {catalog.discoveryOutputTokens ?? "—"}</dd></div> : null}
            </> : null}
          </dl> : null}
        </div>
      </RequestSection> : null}
      {((debugEnabled && hasTools) || toolWarning) && toolUse ? <RequestSection title={t("usage.requestSections.tools")} className="request-tool-details">
        <dl className={`request-details-list${debugEnabled ? " request-tools-list" : ""}`}>
          {debugEnabled ? <>
            <div><dt>{t("usage.clientTools")}</dt><dd>{toolUse.clientToolCount}{!hasContext && toolUse.clientSchemaBytes != null ? ` · ${formatContextBytes(toolUse.clientSchemaBytes, i18n.language)}` : null}</dd></div>
            {toolUse.forwardedToolCount !== toolUse.clientToolCount
              ? <div><dt>{t("usage.forwardedTools")}</dt><dd>{toolUse.forwardedToolCount}</dd></div>
              : null}
            <div><dt>{t("usage.toolCallsReturned")}</dt><dd>{toolUse.toolCallCount}</dd></div>
            {toolUse.requestedParallelToolCalls != null ? <div><dt>{t("usage.requestedParallelTools")}</dt><dd>{t(toolUse.requestedParallelToolCalls ? "common.yes" : "common.no")}</dd></div> : null}
            {toolUse.upstreamToolCallCount != null ? <div><dt>{t("usage.upstreamToolCalls")}</dt><dd>{toolUse.upstreamToolCallCount}</dd></div> : null}
            {toolUse.toolCallsReadyMs != null ? <div><dt>{t("usage.toolCallsReadyTime")}</dt><dd>{formatDurationMs(toolUse.toolCallsReadyMs, i18n.language, t)}</dd></div> : null}
            {toolUse.toolChoice && toolUse.toolChoice !== "auto"
              ? <div><dt>{t("usage.toolChoice")}</dt><dd>{formatToolChoice(toolUse.toolChoice, t)}</dd></div>
              : null}
          </> : null}
          <div><dt>{t("usage.terminalOutput")}</dt><dd className={toolWarning ? "warning-text" : undefined}>
            {toolWarning ? t("usage.toolCallMissing") : formatTerminalOutput(toolUse.terminalOutput, t)}
          </dd></div>
        </dl>
      </RequestSection> : null}
      {row.requestId || row.upstreamError?.requestId || modelAlias || upstreamModel ? <RequestSection title={t("usage.requestSections.identifiers")} className="request-identifiers-section">
        <dl className="request-details-list request-id-list">
          {row.requestId ? <div>
            <dt>{t("usage.localRequestId")}</dt>
            <dd className="request-details-copy-value"><code>{row.requestId}</code><CopyButton value={row.requestId} label={t("usage.copyRequestId")} /></dd>
          </div> : null}
          {row.upstreamError?.requestId ? <div>
            <dt>{t("usage.upstreamRequestId")}</dt>
            <dd className="request-details-copy-value"><code>{row.upstreamError.requestId}</code><CopyButton value={row.upstreamError.requestId} label={t("usage.copyUpstreamRequestId")} /></dd>
          </div> : null}
          {requestedModel ? <div>
            <dt>{t("usage.requestedModel")}</dt>
            <dd className="request-details-copy-value"><code>{requestedModel}</code><CopyButton value={requestedModel} label={t("usage.copyModelId")} /></dd>
          </div> : null}
          {(modelAlias || upstreamModel) && row.routedModel ? <div>
            <dt>{t("usage.routedModel")}</dt><dd><code>{row.routedModel}</code></dd>
          </div> : null}
          {upstreamModel ? <div>
            <dt>{t("usage.upstreamResponseModel")}</dt><dd><code>{upstreamModel}</code></dd>
          </div> : null}
        </dl>
      </RequestSection> : null}
    </div>
    {debugEnabled && routing?.cacheContext
      ? <RequestDisclosure title={t("usage.cacheContext.title")} className="request-context-details">
        {() => <CacheContextDetails diagnostics={routing.cacheContext!} />}
      </RequestDisclosure> : null}
    </>}
  </RequestDisclosure>;
}
