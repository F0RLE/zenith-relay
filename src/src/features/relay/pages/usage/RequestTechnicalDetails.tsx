import { Braces } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { CopyButton } from "../../components/Ui";
import { CacheContextDetails } from "./CacheContextDetails";
import { CacheAffinityDetails } from "./CacheAffinityDetails";
import { RequestDisclosure } from "./RequestDisclosure";
import { RequestSection } from "./RequestSection";
import type { UsageRow } from "./usageData";
import { formatContextBytes, formatDurationMs, formatEndpointKind, formatErrorOrigin, formatModelId, formatObservedServiceTier, formatToolChoice, formatTerminalOutput, formatTransport, missingRequiredToolCall, requestStatusKind } from "./usageReportFormat";

export function RequestTechnicalDetails({ row, debugEnabled }: { row: UsageRow; debugEnabled: boolean }) {
  const { t, i18n } = useTranslation();
  const field = (label: string, value: ReactNode) => <div key={label}><dt>{t(label)}</dt><dd>{value}</dd></div>;
  const copyField = (label: string, value: string, copyLabel: string) => <div key={label}>
    <dt>{t(label)}</dt><dd className="request-details-copy-value"><code>{value}</code><CopyButton value={value} label={t(copyLabel)} /></dd>
  </div>;
  const routing = row.routing;
  const transport = routing?.transport;
  const payload = transport?.requestPayload;
  // Schema validation is included in bodyMs, so preparation parts are not additive.
  const preparationTimings = [
    ["preparationBodyTime", transport?.requestPreparation?.bodyMs],
    ["preparationSchemaTime", transport?.requestPreparation?.schemaValidationMs],
    ["preparationCatalogTime", transport?.requestPreparation?.catalogMs],
    ["preparationSerializationTime", transport?.requestPreparation?.serializationMs],
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
  // BPS embeds tool schemas in upstream instructions.
  const upstreamContext = toolUse?.upstreamContext;
  const hasContext = Boolean(clientContext || upstreamContext);
  const catalog = toolUse?.basisPointsCatalog;
  const contextBytes = (client: number | undefined, upstream: number | undefined) =>
    `${formatContextBytes(client, i18n.language)} → ${formatContextBytes(upstream, i18n.language)}`;
  const observedTier = formatObservedServiceTier(row);
  // Observed model name reported by the provider.
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
            {row.httpStatus != null ? field("usage.httpStatus", row.httpStatus) : null}
            {row.errorOrigin ? field("usage.errorOrigin", formatErrorOrigin(row.errorOrigin, t)) : null}
            {row.errorCategory ? field("usage.errorCategory", <code>{row.errorCategory}</code>) : null}
            {row.upstreamError?.httpStatus != null && row.upstreamError.httpStatus !== row.httpStatus
              ? field("usage.upstreamHttpStatus", row.upstreamError.httpStatus) : null}
            {row.upstreamError?.code && row.upstreamError.code !== row.errorCategory
              ? field("usage.upstreamErrorCode", <code>{row.upstreamError.code}</code>) : null}
            {row.upstreamError?.errorType
              ? field("usage.upstreamErrorType", <code>{row.upstreamError.errorType}</code>) : null}
          </> : null}
          {separateEndpoint && routing?.endpointKind
            ? field("usage.endpoint", formatEndpointKind(routing.endpointKind, row.wireApi, t)) : null}
          {observedTier ? field("usage.upstreamTier", observedTier) : null}
        </dl>
      </RequestSection> : null}
      {debugEnabled && routing ? <RequestSection title={t("usage.requestSections.route")} className={`request-route-section${!payload ? " request-technical-wide" : ""}`}>
        <dl className="request-details-list request-technical-fields">
          <div className="request-route-reason"><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div>
          <CacheAffinityDetails diagnostics={routing.cacheAffinity} />
          {transport ? <>
            {field("usage.transportPath", `${formatTransport(row.transport, t)} → ${formatTransport(transport.upstream, t)}`)}
            {field("usage.transportMode", t(`usage.transportModes.${transport.mode}`))}
            {transport.fallbackReason ? field("usage.transportFallback", t(`usage.transportFallbackReasons.${transport.fallbackReason}`)) : null}
            {transport.responseFormat ? field("usage.upstreamResponseFormat", t(`usage.transportResponseFormats.${transport.responseFormat}`)) : null}
            {transport.handshakeMs != null ? field("usage.handshakeMs", formatDurationMs(transport.handshakeMs, i18n.language, t)) : null}
            {transport.connectionReused != null ? field("usage.connectionReuse", t(transport.connectionReused ? "usage.connectionReused" : "usage.connectionOpened")) : null}
          </> : null}
        </dl>
        <dl className="request-details-list request-route-stats">
          {field("usage.eligibleCandidates", routing.eligibleCandidates)}
          {row.candidateKind === "account" && routing.quotaRemainingBasisPoints != null
            ? field("usage.quotaAtSelection", `${(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%`) : null}
          {field("usage.inFlightAtSelection", routing.inFlightBefore)}
          {field("usage.dispatchesBefore", routing.dispatchesBefore)}
        </dl>
      </RequestSection> : null}
      {debugEnabled && payload ? <RequestSection title={t("usage.requestSections.payload")} className="request-payload-section">
        <dl className="request-details-list request-technical-fields">
          {field("usage.requestPayloadMode", t(`usage.requestPayloadModes.${payload.mode}`))}
          {payload.fullRequestReason ? field("usage.fullRequestReason", t(`usage.fullRequestReasons.${payload.fullRequestReason}`)) : null}
          {field("usage.requestPayloadBytes", contextBytes(payload.jsonBytes, payload.bodyBytes))}
          {field("usage.requestEncoding", payload.contentEncoding === "zstd" ? "Zstd" : t("usage.requestUncompressed"))}
          {payload.compressionMs != null ? field("usage.requestCompressionTime", formatDurationMs(payload.compressionMs, i18n.language, t)) : null}
          {payload.httpSendToHeadersMs != null ? field("usage.requestHttpSendTime", formatDurationMs(payload.httpSendToHeadersMs, i18n.language, t)) : null}
        </dl>
      </RequestSection> : null}
      {debugEnabled && hasTimings ? <RequestSection title={t("usage.requestSections.timings")} className="request-timings-section">
        {hasPreparation ? <div className="request-preparation-details">
          <dl className="request-details-list request-preparation-total">
            {field("usage.requestPreparationTime", transport?.requestPreparationMs != null
              ? formatDurationMs(transport.requestPreparationMs, i18n.language, t) : "—")}
          </dl>
          {hasPreparationTimings ? <dl className="request-details-list request-technical-fields">
            {preparationTimings.map(([label, milliseconds]) => milliseconds != null
              ? field(`usage.${label}`, formatDurationMs(milliseconds, i18n.language, t)) : null)}
          </dl> : null}
        </div> : null}
        {responseTimings.length > 0 ? <div className="request-response-timings">
          <h4>{t("usage.requestSections.afterDispatch")}</h4>
          <dl className="request-details-list request-timing-metrics">
            {responseTimings.map(([label, milliseconds]) => field(`usage.${label}`, `+${formatDurationMs(milliseconds, i18n.language, t)}`))}
          </dl>
        </div> : null}
      </RequestSection> : null}
      {debugEnabled && hasContext ? <RequestSection title={t("usage.contextSize.title")} className={`request-context-section${!hasTools ? " request-technical-wide" : ""}`}>
        <div className="request-context-columns">
          <dl className="request-details-list">
            {(["bodyBytes", "instructionBytes", "conversationBytes", "toolCatalogBytes"] as const).map((name, index) =>
              field(`usage.contextSize.fields.${index}`, contextBytes(clientContext?.[name], upstreamContext?.[name])))}
          </dl>
          {catalog ? <dl className="request-details-list">
            {field("usage.contextSize.catalogMode", t(`usage.contextSize.modes.${catalog.mode}`))}
            {field("usage.contextSize.schemas", contextBytes(catalog.fullSchemaBytes, catalog.renderedSchemaBytes))}
            {field("usage.contextSize.loaded", `${catalog.loadedToolCount} / ${toolUse?.forwardedToolCount ?? 0}`)}
            {catalog.mode === "discovery" ? <>
              {field("usage.contextSize.lookups", catalog.discoveryRounds)}
              {catalog.discoveryLatencyMs != null ? field("usage.contextSize.lookupDuration", formatDurationMs(catalog.discoveryLatencyMs, i18n.language, t)) : null}
              {catalog.discoveryRounds > 0 ? field("usage.contextSize.lookupTokens", `${catalog.discoveryInputTokens ?? "—"} / ${catalog.discoveryOutputTokens ?? "—"}`) : null}
            </> : null}
          </dl> : null}
        </div>
      </RequestSection> : null}
      {((debugEnabled && hasTools) || toolWarning) && toolUse ? <RequestSection title={t("usage.requestSections.tools")} className={`request-tool-details${!debugEnabled || !hasContext ? " request-technical-wide" : ""}`}>
        <dl className={`request-details-list${debugEnabled ? " request-tools-list" : ""}`}>
          {debugEnabled ? <>
            {field("usage.clientTools", <>{toolUse.clientToolCount}{!hasContext && toolUse.clientSchemaBytes != null ? ` · ${formatContextBytes(toolUse.clientSchemaBytes, i18n.language)}` : null}</>)}
            {toolUse.forwardedToolCount !== toolUse.clientToolCount
              ? field("usage.forwardedTools", toolUse.forwardedToolCount)
              : null}
            {field("usage.toolCallsReturned", toolUse.toolCallCount)}
            {toolUse.requestedParallelToolCalls != null ? field("usage.requestedParallelTools", t(toolUse.requestedParallelToolCalls ? "common.yes" : "common.no")) : null}
            {toolUse.upstreamToolCallCount != null ? field("usage.upstreamToolCalls", toolUse.upstreamToolCallCount) : null}
            {toolUse.toolCallsReadyMs != null ? field("usage.toolCallsReadyTime", formatDurationMs(toolUse.toolCallsReadyMs, i18n.language, t)) : null}
            {toolUse.toolChoice && toolUse.toolChoice !== "auto"
              ? field("usage.toolChoice", formatToolChoice(toolUse.toolChoice, t))
              : null}
          </> : null}
          <div><dt>{t("usage.terminalOutput")}</dt><dd className={toolWarning ? "warning-text" : undefined}>
            {toolWarning ? t("usage.toolCallMissing") : formatTerminalOutput(toolUse.terminalOutput, t)}
          </dd></div>
        </dl>
      </RequestSection> : null}
      {row.requestId || row.upstreamError?.requestId || modelAlias || upstreamModel ? <RequestSection title={t("usage.requestSections.identifiers")} className="request-identifiers-section">
        <dl className="request-details-list request-id-list">
          {row.requestId ? copyField("usage.localRequestId", row.requestId, "usage.copyRequestId") : null}
          {row.upstreamError?.requestId ? copyField("usage.upstreamRequestId", row.upstreamError.requestId, "usage.copyUpstreamRequestId") : null}
          {requestedModel ? copyField("usage.requestedModel", requestedModel, "usage.copyModelId") : null}
          {(modelAlias || upstreamModel) && row.routedModel ? field("usage.routedModel", <code>{row.routedModel}</code>) : null}
          {upstreamModel ? field("usage.upstreamResponseModel", <code>{upstreamModel}</code>) : null}
        </dl>
      </RequestSection> : null}
    </div>
    {debugEnabled && routing?.cacheContext
      ? <RequestSection title={t("usage.cacheContext.title")} className="request-context-details">
        <CacheContextDetails diagnostics={routing.cacheContext} />
      </RequestSection> : null}
    </>}
  </RequestDisclosure>;
}
