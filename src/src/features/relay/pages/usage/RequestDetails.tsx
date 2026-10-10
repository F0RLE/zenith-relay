import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { CopyButton, Dialog, StatusBadge, Tabs } from "../../components/Ui";
import { tokenSpeed } from "../../usageSpeed";
import { formatFullNumber } from "../../usageTotals";
import { cacheLifetime } from "./cacheLifetime";
import { CacheContextDetails } from "./CacheContextDetails";
import { usageSpeedSample } from "./usageData";
import type { UsageRow } from "./usageData";
import { usageBreakdown } from "./usageBreakdown";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatDurationMs, requestStatusLabel, formatRequestOrigin, formatReasoningSummary, formatServiceTier, formatObservedServiceTier, formatWireApi, formatTransport, formatEndpointKind, formatErrorCategory, formatErrorOrigin, formatToolChoice, formatTerminalOutput, cacheRemainingLabel, prefixErrorOrigin } from "./usageReportFormat";
import type { CacheTouch } from "./usageReportFormat";
import { RequestDetailMetric, SpeedValue } from "./usageReportParts";

export function RequestDetails({ row, local, onClose }: { row: UsageRow; local: boolean; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [section, setSection] = useState<"overview" | "tokens" | "context" | "tools" | "route">("overview");
  const [debugEnabled, setDebugEnabled] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [cacheTouch, setCacheTouch] = useState<CacheTouch | null>(null);
  const routing = row.routing;
  const toolUse = row.toolUse;
  const speed = usageSpeedSample(row);
  const generationSpeed = tokenSpeed(speed);
  const breakdown = usageBreakdown({
    inputTokens: row.inputTokens,
    cachedInputTokens: row.cachedInputTokens,
    cacheWriteInputTokens: row.cacheWriteInputTokens,
    outputTokens: row.outputTokens,
    reasoningTokens: row.reasoningTokens,
    totalTokens: row.tokens,
  });
  const formatTokens = (tokenCount: number | null) => tokenCount == null ? "—" : formatFullNumber(tokenCount, i18n.language);
  const hasTokenValue = (tokenCount: number | null): tokenCount is number => tokenCount != null && tokenCount > 0;
  const hasCacheRead = hasTokenValue(breakdown.cacheRead);
  const hasCacheWrite = hasTokenValue(breakdown.cacheWrite);
  const showCacheClock = hasCacheRead || hasCacheWrite;
  useEffect(() => {
    let active = true;
    void relayCommands.diagnosticSettings()
      .then((settings) => { if (active) setDebugEnabled(settings.debugEnabled); })
      .catch(() => { if (active) setDebugEnabled(false); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    if (!showCacheClock) return;
    const timer = window.setInterval(() => setNowMs(Date.now()), 15_000);
    return () => window.clearInterval(timer);
  }, [showCacheClock]);
  useEffect(() => {
    if (!showCacheClock) {
      setCacheTouch(null);
      return;
    }
    const own: CacheTouch = { model: row.model, cacheWriteTtl: row.cacheWriteTtl, touchedAt: row.time };
    if (!local || !row.clientContextId) {
      setCacheTouch(own);
      return;
    }
    let active = true;
    setCacheTouch(null);
    relayCommands.localCacheSessions().then((sessions) => {
      if (!active) return;
      const session = sessions.find((cacheSession) => cacheSession.clientContextId === row.clientContextId);
      setCacheTouch(session
        ? { model: session.model ?? row.model, cacheWriteTtl: session.cacheWriteTtl ?? row.cacheWriteTtl, touchedAt: session.touchedAt }
        : own);
    }).catch(() => { if (active) setCacheTouch(own); });
    return () => { active = false; };
  }, [local, row.cacheWriteTtl, row.clientContextId, row.model, row.time, showCacheClock]);
  const cacheRemaining = cacheTouch ? cacheRemainingLabel(cacheLifetime(cacheTouch, nowMs), t) : null;
  const providerMessage = row.upstreamError?.message?.trim();
  const upstreamErrorMessage = providerMessage
    ? prefixErrorOrigin(row.errorOrigin, providerMessage)
    : null;
  const requestCost = row.apiEquivalent && (row.apiEquivalent.microUsd > 0 || row.apiEquivalent.pricedTokens > 0)
    ? formatUsageApiEquivalent(row.apiEquivalent, i18n.language)
    : "—";
  const costHint = t("usage.requestApiEquivalentHint", { count: row.apiEquivalent?.unpricedTokens ?? 0 });
  const costClassName = requestCost !== "—" ? "request-details-cost-value" : undefined;
  const toolWarning = Boolean(
    row.success
      && toolUse
      && toolUse.forwardedToolCount > 0
      && toolUse.toolCallCount === 0
      && toolUse.terminalOutput === "text",
  );
  const tabs = [
    { id: "overview", label: t("usage.requestSections.overview") },
    { id: "tokens", label: t("usage.requestSections.tokens") },
    ...(debugEnabled && routing?.cacheContext ? [{ id: "context", label: t("usage.requestSections.context") }] : []),
    ...(toolUse ? [{ id: "tools", label: t("usage.requestSections.tools") }] : []),
    ...(routing ? [{ id: "route", label: t("usage.requestSections.route") }] : []),
  ];
  const activeSection = tabs.some((tab) => tab.id === section) ? section : "overview";
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} wide className="request-details-dialog">
    <div className="request-details-header">
      <div className="request-details-identity">
        <StatusBadge status={row.requestOrigin?.startsWith("blocked_") ? "warning" : row.success ? "ready" : "error"} label={requestStatusLabel(row, t)} />
        <code data-relay-tooltip={row.model ?? undefined}>{row.model ?? "-"}</code>
      </div>
      <div className="request-details-id">
        <span>{t("usage.localRequestId")}</span>
        <code data-relay-tooltip={row.requestId ?? undefined}>{row.requestId ?? "-"}</code>
        {row.requestId ? <CopyButton value={row.requestId} label={t("usage.copyRequestId")} /> : null}
      </div>
    </div>
    <div className="request-details-metrics">
      <RequestDetailMetric label={t("usage.totalTime")} value={formatDurationMs(row.duration, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.firstResponse")} value={formatDurationMs(row.ttft, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.generationSpeed")} value={<SpeedValue value={generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} />} />
      <RequestDetailMetric label={t("usage.requestCost")} value={<span className={costClassName} data-relay-tooltip={costHint}>{requestCost}</span>} />
    </div>
    <Tabs value={activeSection} items={tabs} onChange={(selectedSection) => setSection(selectedSection as typeof section)} label={t("usage.requestSectionsLabel")} />
    {activeSection === "overview" ? <>
      <dl className="request-details-list">
        {row.requestedModel && row.routedModel && row.requestedModel !== row.routedModel ? <>
          <div><dt>{t("usage.requestedModel")}</dt><dd><code>{row.requestedModel}</code></dd></div>
          <div><dt>{t("usage.routedModel")}</dt><dd><code>{row.routedModel}</code></dd></div>
        </> : null}
        <div><dt>{t("usage.poolMember")}</dt><dd>{row.connection}</dd></div>
        <div><dt>{t("usage.protocol")}</dt><dd><code>{formatWireApi(row.wireApi, t)}</code></dd></div>
        <div><dt>{t("usage.transport")}</dt><dd><code>{formatTransport(row.transport, t)}</code></dd></div>
        {routing?.endpointKind && routing.endpointKind !== row.wireApi ? (
          <div><dt>{t("usage.endpoint")}</dt><dd><code>{formatEndpointKind(routing.endpointKind, row.wireApi, t)}</code></dd></div>
        ) : null}
        <div><dt>{t("usage.serviceTier")}</dt><dd>{formatServiceTier(row, t, "-")}</dd></div>
        {formatObservedServiceTier(row) ? <div><dt>{t("usage.upstreamTier")}</dt><dd><code>{formatObservedServiceTier(row)}</code></dd></div> : null}
        <div><dt>{t("usage.reasoning")}</dt><dd>{formatReasoningSummary(row, t)}</dd></div>
        {row.requestOrigin ? <div><dt>{t("usage.requestOrigin")}</dt><dd data-relay-tooltip={t("codex.backgroundRequestHint")}>{formatRequestOrigin(row.requestOrigin, t)}</dd></div> : null}
      </dl>
      {!row.success ? <section className="request-details-error" aria-label={t("usage.errorDetails")}>
        <h3>{t("usage.errorDetails")}</h3>
        <dl className="request-details-list">
          {row.attempt >= 2 ? <div><dt>{t("usage.attempt")}</dt><dd>{row.attempt}</dd></div> : null}
          <div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div>
          <div><dt>{t("usage.errorOrigin")}</dt><dd>{formatErrorOrigin(row.errorOrigin, t)}</dd></div>
          <div><dt>{t("usage.errorCategory")}</dt><dd data-relay-tooltip={row.errorCategory ?? undefined}>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : "-"}</dd></div>
          {row.upstreamError?.requestId ? <div>
            <dt>{t("usage.upstreamRequestId")}</dt>
            <dd className="request-details-copy-value"><code>{row.upstreamError.requestId}</code><CopyButton value={row.upstreamError.requestId} label={t("usage.copyUpstreamRequestId")} /></dd>
          </div> : null}
          {row.upstreamError?.httpStatus != null && row.upstreamError.httpStatus !== row.httpStatus
            ? <div><dt>{t("usage.upstreamHttpStatus")}</dt><dd>{row.upstreamError.httpStatus}</dd></div>
            : null}
          {row.upstreamError?.code && row.upstreamError.code !== row.errorCategory
            ? <div><dt>{t("usage.upstreamErrorCode")}</dt><dd><code>{row.upstreamError.code}</code></dd></div>
            : null}
          {row.upstreamError?.errorType
            ? <div><dt>{t("usage.upstreamErrorType")}</dt><dd><code>{row.upstreamError.errorType}</code></dd></div>
            : null}
        </dl>
        {row.upstreamError && upstreamErrorMessage ? <>
          <div className="request-upstream-message">
            <pre>{upstreamErrorMessage}</pre>
            <CopyButton value={upstreamErrorMessage} label={t("usage.copyUpstreamError")} />
          </div>
          {row.upstreamError.redacted ? <p className="form-note">{t("usage.upstreamErrorRedacted")}</p> : null}
          {row.upstreamError.truncated ? <p className="form-note">{t("usage.upstreamErrorTruncated")}</p> : null}
        </> : null}
      </section> : null}
    </> : null}
    {activeSection === "tokens" ? <dl className="request-details-list request-details-token-list">
      {breakdown.inputTotal == null || hasTokenValue(breakdown.inputTotal) ? <div className="request-details-token-group">
        <div className="request-details-token-group-heading"><dt>{t("usage.inputTokens")}</dt><dd>{formatTokens(breakdown.inputTotal)}</dd></div>
        {hasTokenValue(breakdown.uncachedInput) ? <div className="request-details-token-child"><dt>{t("usage.uncachedInputTokens")}</dt><dd>{formatTokens(breakdown.uncachedInput)}</dd></div> : null}
        {hasCacheRead ? (
          <div className="request-details-token-child">
            <dt>{t("usage.cachedInputTokens")}</dt>
            <dd>{formatTokens(breakdown.cacheRead)}{cacheRemaining ? ` (${cacheRemaining})` : ""}</dd>
          </div>
        ) : null}
        {hasCacheWrite ? (
          <div className="request-details-token-child">
            <dt>{t("usage.cacheWriteInputTokens")}</dt>
            <dd>{formatTokens(breakdown.cacheWrite)}{!hasCacheRead && cacheRemaining ? ` (${cacheRemaining})` : ""}</dd>
          </div>
        ) : null}
      </div> : null}
      {breakdown.outputTotal == null || hasTokenValue(breakdown.outputTotal) ? <div className="request-details-token-group">
        <div className="request-details-token-group-heading"><dt>{t("usage.outputTokens")}</dt><dd>{formatTokens(breakdown.outputTotal)}</dd></div>
        {hasTokenValue(breakdown.reasoning) ? <div className="request-details-token-child"><dt>{t("usage.reasoningTokens")}</dt><dd>{formatTokens(breakdown.reasoning)}</dd></div> : null}
      </div> : null}
      {breakdown.total == null || hasTokenValue(breakdown.total) ? <div className="request-details-token-total"><dt>{t("usage.totalTokens")}</dt><dd>{formatTokens(breakdown.total)}</dd></div> : null}
      <div className="request-details-token-cost"><dt>{t("usage.requestCost")}</dt><dd><span className={costClassName} data-relay-tooltip={costHint}>{requestCost}</span></dd></div>
    </dl> : null}
    {activeSection === "context" ? <CacheContextDetails diagnostics={routing?.cacheContext} /> : null}
    {activeSection === "tools" && toolUse ? <section className="request-details-section">
      <dl className="request-details-list">
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
    </section> : null}
    {activeSection === "route" && routing ? <dl className="request-details-list">
      <div><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div>
      <div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div>
      {row.candidateKind === "account" ? (
        <div>
          <dt>{t("usage.quotaAtSelection")}</dt>
          <dd>{routing.quotaRemainingBasisPoints == null ? t("common.unknown") : `${(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%`}</dd>
        </div>
      ) : null}
      <div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div>
      <div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div>
    </dl> : null}
  </Dialog>;
}
