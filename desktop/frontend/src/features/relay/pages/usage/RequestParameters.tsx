import { useTranslation } from "react-i18next";
import type { UsageRow } from "./usageData";
import {
  formatReasoningEffort, formatReasoningSummary, formatRequestOrigin,
  formatServiceTier, formatTransport, formatWireApi,
} from "./usageReportFormat";

export function hasRequestParameters(row: UsageRow) {
  return Boolean(row.wireApi || row.transport || row.serviceTier || row.effectiveReasoningEffort || row.requestedReasoningEffort || row.requestOrigin);
}

export function RequestParameters({ row }: { row: UsageRow }) {
  const { t } = useTranslation();
  const requestedEffort = row.requestedReasoningEffort;
  const effectiveEffort = row.effectiveReasoningEffort;
  const reasoningChanged = requestedEffort && effectiveEffort && requestedEffort !== effectiveEffort;
  if (!hasRequestParameters(row)) return null;
  return <dl className="request-details-parameters" aria-label={t("usage.requestSections.parameters")}>
    {row.wireApi ? <div><dt>{t("usage.protocol")}</dt><dd>{row.imageUsage ? "Images API" : formatWireApi(row.wireApi, t)}</dd></div> : null}
    {row.transport ? <div><dt>{t("usage.transport")}</dt><dd>{formatTransport(row.transport, t)}</dd></div> : null}
    {row.serviceTier ? <div><dt>{t("usage.serviceTier")}</dt><dd>{formatServiceTier(row, t)}</dd></div> : null}
    {requestedEffort || effectiveEffort ? <div>
      <dt>{t("usage.reasoning")}</dt>
      <dd data-relay-tooltip={reasoningChanged ? formatReasoningSummary(row, t) : undefined}>
        {reasoningChanged
          ? <><span className="request-parameter-requested">{formatReasoningEffort(requestedEffort, t)}</span><span className="request-parameter-result"><span className="request-parameter-arrow" aria-hidden>→</span>{formatReasoningEffort(effectiveEffort, t)}</span></>
          : formatReasoningEffort(effectiveEffort ?? requestedEffort, t)}
      </dd>
    </div> : null}
    {row.requestOrigin ? <div><dt>{t("usage.requestOrigin")}</dt><dd>{formatRequestOrigin(row.requestOrigin, t)}</dd></div> : null}
  </dl>;
}
