import { useEffect, useState } from "react";
import { CircleAlert, CircleCheck, Clock3, Link2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { CopyButton, Dialog } from "../../components/Ui";
import { tokenSpeed } from "../../usageSpeed";
import { formatFullNumber } from "../../usageTotals";
import { cacheLifetime, cacheTouchForRow } from "./cacheLifetime";
import type { CacheSessionRecord } from "../../api/types";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { usageSpeedSample } from "./usageData";
import type { UsageRow } from "./usageData";
import { usageBreakdown } from "./usageBreakdown";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatDurationMs, requestStatusKind, requestStatusLabel, formatErrorCategory, cacheRemainingLabel, prefixErrorOrigin } from "./usageReportFormat";
import { RequestDetailMetric, RequestTokenValue, SpeedValue } from "./usageReportParts";
import { RequestParameters } from "./RequestParameters";
import { RequestTechnicalDetails } from "./RequestTechnicalDetails";

export function RequestDetails({ row, local, onClose }: {
  row: UsageRow; local: boolean; onClose: () => void;
}) {
  const { t, i18n } = useTranslation();
  const { usageRevision } = useRelayUsageContext();
  const [debugEnabled, setDebugEnabled] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [cacheSessions, setCacheSessions] = useState<CacheSessionRecord[]>([]);
  const cacheTouch = cacheTouchForRow(row, local ? cacheSessions : []);
  const cacheStatus = cacheTouch ? cacheLifetime(cacheTouch, nowMs) : null;
  const routing = row.routing;
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
  const prices = row.apiEquivalentBreakdown;
  const showCacheCounters = breakdown.cacheRead != null || breakdown.cacheWrite != null || cacheTouch != null;
  const showCacheClock = cacheStatus?.expiry === "open";
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
    if (!local || !row.clientContextId) return;
    let active = true;
    void relayCommands.localCacheSessions()
      .then((sessions) => { if (active) setCacheSessions(sessions); })
      // A failed read cannot erase an earlier observation or extend its lifetime.
      .catch(() => undefined);
    return () => { active = false; };
  }, [local, row.clientContextId, row.candidateKind, row.candidateKey, row.routedModel, row.requestedModel, usageRevision]);
  const cacheRemaining = cacheStatus && (cacheStatus.expiry !== "unknown" || cacheStatus.windows.length > 1)
    ? cacheRemainingLabel(cacheStatus, t) : null;
  const providerMessage = row.upstreamError?.message?.trim();
  const upstreamErrorMessage = providerMessage
    ? prefixErrorOrigin(row.errorOrigin, providerMessage)
    : null;
  const apiEquivalent = row.apiEquivalent && (row.apiEquivalent.microUsd > 0 || row.apiEquivalent.pricedTokens > 0)
    ? formatUsageApiEquivalent(row.apiEquivalent, i18n.language, 6)
    : "—";
  const equivalentHint = t("usage.requestApiEquivalentHint", { count: row.apiEquivalent?.unpricedTokens ?? 0 });
  const hasTokens = [row.inputTokens, row.outputTokens, row.tokens, row.cachedInputTokens,
    row.cacheWriteInputTokens, row.reasoningTokens].some((value) => value != null);
  const stopDecision = !row.success && (routing?.attemptOutcome === "execution_uncertain"
    || routing?.attemptOutcome === "client_disconnected") ? routing.attemptOutcome : null;
  const requestTimestamp = Date.parse(row.time);
  const requestTime = Number.isFinite(requestTimestamp)
    ? new Intl.DateTimeFormat(i18n.resolvedLanguage ?? i18n.language, { dateStyle: "short", timeStyle: "medium" }).format(requestTimestamp)
    : null;
  const requestStatus = requestStatusKind(row);
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} className="request-details-dialog">
    <div className="request-overview">
      <div className="request-details-header">
        <div className="request-details-identity">
          <div className="request-details-model">
            <span className="request-details-model-label">{t("common.model")}</span>
            <h3>{row.model ?? t("common.unknown")}</h3>
          </div>
          <span className="request-details-status" data-status={requestStatus}>
            {requestStatus === "success" ? <CircleCheck aria-hidden /> : requestStatus === "pending" ? <Clock3 aria-hidden /> : <CircleAlert aria-hidden />}
            {requestStatusLabel(row, t)}
          </span>
        </div>
        <dl className="request-details-subtitle">
          <div><dt><Link2 aria-hidden />{t("usage.requestConnection")}</dt><dd>{row.connection}</dd></div>
          {requestTime ? <div><dt><Clock3 aria-hidden />{t("usage.time")}</dt><dd><time dateTime={row.time}>{requestTime}</time></dd></div> : null}
        </dl>
      </div>
      <dl className="request-details-metrics">
        {row.ttft != null ? <RequestDetailMetric label={t("usage.firstResponse")} value={formatDurationMs(row.ttft, i18n.resolvedLanguage ?? i18n.language, t)} /> : null}
        <RequestDetailMetric label={t("usage.responseTime")} value={formatDurationMs(row.duration, i18n.resolvedLanguage ?? i18n.language, t)} />
        {generationSpeed != null ? <RequestDetailMetric label={t("usage.generationSpeed")} value={<SpeedValue value={generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} />} /> : null}
        {apiEquivalent !== "—" ? <RequestDetailMetric label={t("usage.apiEquivalent")} value={<span className="request-details-cost-value" data-relay-tooltip={equivalentHint}>{apiEquivalent}</span>} /> : null}
      </dl>
    </div>
    {!row.success && requestStatus !== "pending" ? <section className="request-details-error" aria-label={t("usage.errorDetails")}>
      <div className="request-outcome-heading">
        <CircleAlert aria-hidden />
        <h3>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : t("common.failed")}</h3>
        {row.httpStatus != null ? <code>HTTP {row.httpStatus}</code> : null}
      </div>
      {stopDecision ? <p className="request-outcome-note">{t(`usage.execution.stops.${stopDecision}`)}</p> : null}
      {row.upstreamError && upstreamErrorMessage ? <div className="request-provider-response">
        <div className="request-upstream-message">
          <pre>{upstreamErrorMessage}</pre>
          <CopyButton value={upstreamErrorMessage} label={t("usage.copyUpstreamError")} />
        </div>
        {row.upstreamError.redacted ? <p className="form-note">{t("usage.upstreamErrorRedacted")}</p> : null}
        {row.upstreamError.truncated ? <p className="form-note">{t("usage.upstreamErrorTruncated")}</p> : null}
      </div> : null}
    </section> : null}
    <div className="request-details-content">
      {hasTokens ? <section className="request-detail-section request-token-details" aria-label={t("usage.requestSections.tokens")}>
        {breakdown.total != null ? <dl className="request-token-total">
          <div><dt>{t("usage.totalTokens")}</dt><dd>{formatTokens(breakdown.total)}</dd></div>
        </dl> : null}
        <dl className="request-details-list request-details-token-list">
          <div className="request-details-token-group">
            <div className="request-details-token-group-heading"><dt>{t("usage.inputTokens")}</dt><RequestTokenValue tokens={breakdown.inputTotal} price={prices?.input} /></div>
            {hasTokenValue(breakdown.uncachedInput) ? <div className="request-details-token-child"><dt>{t("usage.uncachedInputTokens")}</dt><RequestTokenValue tokens={breakdown.uncachedInput} price={prices?.uncachedInput} /></div> : null}
            {showCacheCounters ? <>
              <div className="request-details-token-child">
                <dt>{t("usage.cachedInputTokens")}</dt>
                <RequestTokenValue tokens={breakdown.cacheRead} price={prices?.cacheRead} />
              </div>
              <div className="request-details-token-child">
                <dt>{t("usage.cacheWriteInputTokens")}</dt>
                <RequestTokenValue tokens={breakdown.cacheWrite} price={prices?.cacheWrite} />
              </div>
            </> : null}
            {cacheRemaining ? <div className="request-cache-retention"><dt>{t("usage.cacheReport.retention")}</dt><dd>{cacheRemaining}</dd></div> : null}
          </div>
          <div className="request-details-token-group">
            <div className="request-details-token-group-heading"><dt>{t("usage.outputTokens")}</dt><RequestTokenValue tokens={breakdown.outputTotal} price={prices?.output} /></div>
            {hasTokenValue(breakdown.reasoning) && breakdown.visibleOutput != null ? <div className="request-details-token-child"><dt>{t("usage.visibleOutputTokens")}</dt><RequestTokenValue tokens={breakdown.visibleOutput} included /></div> : null}
            {hasTokenValue(breakdown.reasoning) ? <div className="request-details-token-child"><dt>{t("usage.reasoningTokens")}</dt><RequestTokenValue tokens={breakdown.reasoning} included /></div> : null}
          </div>
        </dl>
      </section> : null}
      <RequestParameters row={row} />
    </div>
    <RequestTechnicalDetails row={row} debugEnabled={debugEnabled} />
  </Dialog>;
}
