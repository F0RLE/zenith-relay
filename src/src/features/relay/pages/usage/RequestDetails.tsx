import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { CopyButton, Dialog, StatusBadge, Tabs } from "../../components/Ui";
import { tokenSpeed } from "../../usageSpeed";
import { formatCompactNumber, formatFullNumber } from "../../usageTotals";
import { cacheLifetime } from "./cacheLifetime";
import { usageSpeedSample } from "./usageData";
import type { UsageRow } from "./usageData";
import { usageBreakdown } from "./usageBreakdown";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatDurationMs, requestStatusLabel, formatRequestOrigin, formatReasoningSummary, formatServiceTier, formatObservedServiceTier, formatWireApi, formatEndpointKind, formatErrorCategory, formatErrorOrigin, formatToolChoice, formatTerminalOutput, cacheRemainingLabel } from "./usageReportFormat";
import type { CacheTouch } from "./usageReportFormat";
import { RequestDetailMetric, SpeedValue } from "./usageReportParts";

export function RequestDetails({ row, local, onClose }: { row: UsageRow; local: boolean; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [section, setSection] = useState<"overview" | "tokens" | "tools" | "route">("overview");
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
  const formatTokens = (value: number | null) => value == null ? "—" : formatFullNumber(value, i18n.language);
  const hasTokenValue = (value: number | null): value is number => value != null && value > 0;
  const hasCacheRead = hasTokenValue(breakdown.cacheRead);
  const hasCacheWrite = hasTokenValue(breakdown.cacheWrite);
  const showCacheClock = hasCacheRead || hasCacheWrite;
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
      const session = sessions.find((item) => item.clientContextId === row.clientContextId);
      setCacheTouch(session
        ? { model: session.model ?? row.model, cacheWriteTtl: session.cacheWriteTtl ?? row.cacheWriteTtl, touchedAt: session.touchedAt }
        : own);
    }).catch(() => { if (active) setCacheTouch(own); });
    return () => { active = false; };
  }, [local, row.cacheWriteTtl, row.clientContextId, row.model, row.time, showCacheClock]);
  const cacheRemaining = cacheTouch ? cacheRemainingLabel(cacheLifetime(cacheTouch, nowMs), t) : null;
  const toolWarning = Boolean(
    toolUse
      && toolUse.forwardedToolCount > 0
      && toolUse.toolCallCount === 0
      && toolUse.terminalOutput === "text",
  );
  const tabs = [
    { id: "overview", label: t("usage.requestSections.overview") },
    { id: "tokens", label: t("usage.requestSections.tokens") },
    ...(toolUse ? [{ id: "tools", label: t("usage.requestSections.tools") }] : []),
    ...(routing ? [{ id: "route", label: t("usage.requestSections.route") }] : []),
  ];
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} wide className="request-details-dialog">
    <div className="request-details-header">
      <div className="request-details-identity">
        <StatusBadge status={row.requestOrigin?.startsWith("blocked_") ? "warning" : row.success ? "ready" : "error"} label={requestStatusLabel(row, t)} />
        <code data-relay-tooltip={row.model ?? undefined}>{row.model ?? "-"}</code>
      </div>
      <div className="request-details-id">
        <span>{t("usage.requestId")}</span>
        <code data-relay-tooltip={row.requestId ?? undefined}>{row.requestId ?? "-"}</code>
        {row.requestId ? <CopyButton value={row.requestId} label={t("usage.copyRequestId")} /> : null}
      </div>
    </div>
    <div className="request-details-metrics">
      <RequestDetailMetric label={t("usage.firstResponse")} value={formatDurationMs(row.ttft, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.generationSpeed")} value={<SpeedValue value={generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} />} />
      <RequestDetailMetric label={t("usage.totalTime")} value={formatDurationMs(row.duration, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.visibleOutputTokens")} value={breakdown.visibleOutput == null ? "—" : formatCompactNumber(breakdown.visibleOutput, i18n.language)} />
    </div>
    <Tabs value={section} items={tabs} onChange={(value) => setSection(value as typeof section)} label={t("usage.requestSectionsLabel")} />
    {section === "overview" ? <>
      <dl className="request-details-list">
        {row.requestedModel && row.routedModel && row.requestedModel !== row.routedModel ? <>
          <div><dt>{t("usage.requestedModel")}</dt><dd><code>{row.requestedModel}</code></dd></div>
          <div><dt>{t("usage.routedModel")}</dt><dd><code>{row.routedModel}</code></dd></div>
        </> : null}
        <div><dt>{t("usage.poolMember")}</dt><dd>{row.connection}</dd></div>
        <div><dt>{t("usage.protocol")}</dt><dd><code>{formatWireApi(row.wireApi, t)}</code></dd></div>
        {row.success && routing?.endpointKind && routing.endpointKind !== row.wireApi ? (
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
          <div><dt>{t("usage.attempt")}</dt><dd>{row.attempt}</dd></div>
          <div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div>
          <div><dt>{t("usage.errorOrigin")}</dt><dd>{formatErrorOrigin(row.errorOrigin, t)}</dd></div>
          <div><dt>{t("usage.errorCategory")}</dt><dd data-relay-tooltip={row.errorCategory ?? undefined}>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : "-"}</dd></div>
          <div><dt>{t("usage.endpoint")}</dt><dd><code>{formatEndpointKind(routing?.endpointKind, row.wireApi, t)}</code></dd></div>
        </dl>
        <h3>{t("usage.upstreamError")}</h3>
        {row.upstreamError ? <>
          <dl className="request-details-list request-provider-fields">
            <div><dt>{t("usage.upstreamHttpStatus")}</dt><dd>{row.upstreamError.httpStatus ?? t("common.unknown")}</dd></div>
            {row.upstreamError.code ? <div><dt>{t("usage.upstreamErrorCode")}</dt><dd><code>{row.upstreamError.code}</code></dd></div> : null}
            {row.upstreamError.errorType ? <div><dt>{t("usage.upstreamErrorType")}</dt><dd><code>{row.upstreamError.errorType}</code></dd></div> : null}
          </dl>
          {row.upstreamError.message ? <div className="request-upstream-message">
            <pre>{row.upstreamError.message}</pre>
            <CopyButton value={row.upstreamError.message} label={t("usage.copyUpstreamError")} />
          </div> : <p className="form-note">{t("usage.upstreamErrorUnavailable")}</p>}
          {row.upstreamError.redacted ? <p className="form-note">{t("usage.upstreamErrorRedacted")}</p> : null}
          {row.upstreamError.truncated ? <p className="form-note">{t("usage.upstreamErrorTruncated")}</p> : null}
        </> : <p className="form-note">{t("usage.upstreamErrorUnavailable")}</p>}
      </section> : null}
    </> : null}
    {section === "tokens" ? <dl className="request-details-list request-details-token-list">
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
      {row.apiEquivalent && (row.apiEquivalent.microUsd > 0 || row.apiEquivalent.pricedTokens > 0 || row.apiEquivalent.unpricedTokens > 0) ? (
        <div className="request-details-token-api">
          <dt>{t("usage.apiEquivalent")}</dt>
          <dd data-relay-tooltip={t("usage.requestApiEquivalentHint", { count: row.apiEquivalent.unpricedTokens })}>
            {formatUsageApiEquivalent(row.apiEquivalent, i18n.language)}
          </dd>
        </div>
      ) : null}
    </dl> : null}
    {section === "tools" && toolUse ? <section className="request-details-section">
      <dl className="request-details-list">
        <div><dt>{t("usage.clientTools")}</dt><dd>{toolUse.clientToolCount} → {toolUse.forwardedToolCount}</dd></div>
        {toolUse.policyMode ? <div><dt>{t("toolPolicy.title")}</dt><dd>{t(`toolPolicy.modes.${toolUse.policyMode}`)}</dd></div> : null}
        {toolUse.policyOutcome ? <div><dt>{t("toolPolicy.result")}</dt><dd>{t(`toolPolicy.outcomes.${toolUse.policyOutcome}`)}</dd></div> : null}
        {toolUse.deferredToolSearch ? <div><dt>{t("toolPolicy.deferred")}</dt><dd>{t("toolPolicy.deferredValue")}</dd></div> : null}
        {toolUse.policyFallback ? <div><dt>{t("toolPolicy.fallback")}</dt><dd>{t("toolPolicy.fallbackValue")}</dd></div> : null}
        {toolUse.filteredToolCount != null && toolUse.filteredToolCount > 0 ? <div><dt>{t("toolPolicy.filtered")}</dt><dd>{toolUse.filteredToolCount}</dd></div> : null}
        {toolUse.clientSchemaBytes != null && toolUse.forwardedSchemaBytes != null ? (
          <div>
            <dt>{t("toolPolicy.schemaBytes")}</dt>
            <dd>{formatFullNumber(toolUse.clientSchemaBytes, i18n.language)} → {formatFullNumber(toolUse.forwardedSchemaBytes, i18n.language)}</dd>
          </div>
        ) : null}
        <div><dt>{t("usage.toolChoice")}</dt><dd>{formatToolChoice(toolUse.toolChoice, t)}</dd></div>
        <div><dt>{t("usage.toolCallsReturned")}</dt><dd>{toolUse.toolCallCount}</dd></div>
        <div><dt>{t("usage.terminalOutput")}</dt><dd>{formatTerminalOutput(toolUse.terminalOutput, t)}</dd></div>
      </dl>
      {toolWarning ? <p className="form-note warning-text">{t("usage.toolCallMissing", { count: toolUse.forwardedToolCount })}</p> : null}
      <p className="form-note">{t("usage.toolDiagnosticsHint")}</p>
    </section> : null}
    {section === "route" && routing ? <dl className="request-details-list">
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
