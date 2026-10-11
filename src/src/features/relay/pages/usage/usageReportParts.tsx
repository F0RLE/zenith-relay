import type { ReactNode } from "react";
import { Bot } from "lucide-react";
import { useTranslation } from "react-i18next";
import { formatTokenSpeed } from "../../usageSpeed";
import { formatCompactNumber, formatFullNumber } from "../../usageTotals";
import type { UsageRow } from "./usageData";
import { formatErrorCategory, formatReasoningEffort } from "./usageReportFormat";

export function RequestDetailMetric({ label, value }: { label: string; value: ReactNode }) {
  return <div className="request-details-metric"><dt>{label}</dt><dd>{value}</dd></div>;
}

export function SpeedValue({ value, locale, unit }: { value: number | null; locale: string; unit: string }) {
  const { t } = useTranslation();
  return <span className="usage-speed-value" data-relay-tooltip={t("usage.generationSpeedHint")}>{formatTokenSpeed(value, locale, unit)}</span>;
}

export function UsageModel({ row }: { row: Pick<UsageRow, "model" | "requestedReasoningEffort" | "effectiveReasoningEffort" | "requestOrigin"> }) {
  const { t } = useTranslation();
  const requested = row.requestedReasoningEffort;
  const effective = row.effectiveReasoningEffort;
  const effort = effective ?? requested;
  const changed = Boolean(requested && effective && requested !== effective);
  const requestOrigin = row.requestOrigin;
  return <span className="usage-model-value">
    <code>{row.model ?? "-"}</code>
    {requestOrigin ? <span className="usage-request-origin" data-relay-tooltip={t("codex.backgroundRequestHint")}><Bot aria-hidden /></span> : null}
    {effort ? <small data-relay-tooltip={changed ? t("usage.reasoningEffortChanged", { requested: formatReasoningEffort(requested, t), effective: formatReasoningEffort(effective, t) }) : undefined}>{changed ? `${formatReasoningEffort(requested, t)} → ${formatReasoningEffort(effective, t)}` : formatReasoningEffort(effort, t)}</small> : null}
  </span>;
}

export function UsageConnection({ row }: { row: Pick<UsageRow, "connection" | "success" | "errorCategory" | "requestOrigin"> }) {
  const { t } = useTranslation();
  return <span className="usage-connection-value">
    <span>{row.connection}</span>
    {!row.success && row.errorCategory && !row.requestOrigin
      ? <small className="error-text">{formatErrorCategory(row.errorCategory, t)}</small> : null}
  </span>;
}

export function CompactNumber({ value, locale }: { value: number; locale: string }) {
  return <span data-relay-tooltip={formatFullNumber(value, locale)}>{formatCompactNumber(value, locale)}</span>;
}
