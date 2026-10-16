import { useTranslation } from "react-i18next";
import type { UsageRow } from "./usageData";
import { RequestSection } from "./RequestSection";
import {
  formatModelId, formatReasoningEffort, formatReasoningSummary, formatRequestOrigin,
  formatServiceTier, formatTransport, formatWireApi,
} from "./usageReportFormat";

export function RequestParameters({ row }: { row: UsageRow }) {
  const { t } = useTranslation();
  const requestedEffort = row.requestedReasoningEffort;
  const effectiveEffort = row.effectiveReasoningEffort;
  const reasoningChanged = requestedEffort && effectiveEffort && requestedEffort !== effectiveEffort;
  const requestedModel = formatModelId(row.requestedModel, row.routedModel);
  const routedModel = formatModelId(row.routedModel);
  const modelChanged = requestedModel !== "—" && routedModel !== "—" && requestedModel !== routedModel;
  if (!row.wireApi && !row.transport && !row.serviceTier && !requestedEffort && !effectiveEffort && !modelChanged && !row.requestOrigin) return null;
  return <RequestSection title={t("usage.requestSections.parameters")} className="request-parameters-section">
    <dl className="request-details-parameters">
    {modelChanged ? <>
      <div><dt>{t("usage.requestedModel")}</dt><dd><code>{requestedModel}</code></dd></div>
      <div><dt>{t("usage.routedModel")}</dt><dd><code>{routedModel}</code></dd></div>
    </> : null}
    {row.wireApi ? <div><dt>{t("usage.protocol")}</dt><dd>{formatWireApi(row.wireApi, t)}</dd></div> : null}
    {row.transport ? <div><dt>{t("usage.transport")}</dt><dd>{formatTransport(row.transport, t)}</dd></div> : null}
    {row.serviceTier ? <div><dt>{t("usage.serviceTier")}</dt><dd>{formatServiceTier(row, t)}</dd></div> : null}
    {requestedEffort || effectiveEffort ? <div>
      <dt>{t("usage.reasoning")}</dt>
      <dd data-relay-tooltip={reasoningChanged ? formatReasoningSummary(row, t) : undefined}>
        {reasoningChanged
          ? <><span>{formatReasoningEffort(requestedEffort, t)}</span><span className="request-parameter-arrow" aria-hidden>→</span><span>{formatReasoningEffort(effectiveEffort, t)}</span></>
          : formatReasoningEffort(effectiveEffort ?? requestedEffort, t)}
      </dd>
    </div> : null}
    {row.requestOrigin ? <div><dt>{t("usage.requestOrigin")}</dt><dd>{formatRequestOrigin(row.requestOrigin, t)}</dd></div> : null}
    </dl>
  </RequestSection>;
}
