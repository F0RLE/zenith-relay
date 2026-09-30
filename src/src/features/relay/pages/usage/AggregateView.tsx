import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { UsageGroup } from "../../api/types";
import { EmptyState } from "../../components/Ui";
import { formatCompactNumber, formatFullNumber } from "../../usageTotals";
import { CONNECTION_COLUMN_IDS, MODEL_COLUMN_IDS, reorderColumns, shiftColumn, useColumnDrag, useStoredColumnOrder } from "./useColumnLayout";
import type { AggregateColumnId } from "./useColumnLayout";
import type { UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatTiming, aggregateRowsFromUsage, aggregateRowFromTotals } from "./usageReportFormat";
import type { AggregateRow } from "./usageReportFormat";
import { SpeedValue, CompactNumber } from "./usageReportParts";

function TokenBreakdown({ group, locale }: { group: AggregateRow; locale: string }) {
  const { t } = useTranslation();
  return <div className="usage-token-breakdown">
    <span data-relay-tooltip={`${t("usage.inputTokens")}: ${formatFullNumber(group.inputTokens, locale)}`}>
      <small>{t("usage.inputShort")}</small>{formatCompactNumber(group.inputTokens, locale)}
    </span>
    <span data-relay-tooltip={`${t("usage.outputTokens")}: ${formatFullNumber(group.outputTokens, locale)}`}>
      <small>{t("usage.outputShort")}</small>{formatCompactNumber(group.outputTokens, locale)}
    </span>
    <span data-relay-tooltip={`${t("usage.cachedInputTokens")}: ${group.cachedInputSamples ? formatFullNumber(group.cachedInputTokens, locale) : t("common.unknown")}`}>
      <small>{t("usage.cachedShort")}</small>{group.cachedInputSamples ? formatCompactNumber(group.cachedInputTokens, locale) : "—"}
    </span>
    {group.cacheWriteInputSamples ? <span data-relay-tooltip={`${t("usage.cacheWriteInputTokens")}: ${formatFullNumber(group.cacheWriteInputTokens, locale)}`}>
      <small>{t("usage.cacheWriteShort")}</small>{formatCompactNumber(group.cacheWriteInputTokens, locale)}
    </span> : null}
    <span data-relay-tooltip={`${t("usage.reasoningTokens")}: ${formatFullNumber(group.reasoningTokens, locale)}`}>
      <small>{t("usage.reasoningShort")}</small>{formatCompactNumber(group.reasoningTokens, locale)}
    </span>
  </div>;
}

export function AggregateView({ rows, groups, field, empty }: { rows: UsageRow[]; groups?: UsageGroup[]; field: "model" | "connection"; empty: string }) {
  const { t, i18n } = useTranslation();
  const aggregateRows = groups?.map(({ key, label, totals }) => aggregateRowFromTotals(label || key || t("common.unknown"), totals)) ?? aggregateRowsFromUsage(rows, field, t("common.unknown"));
  const defaults: readonly AggregateColumnId[] = field === "model" ? MODEL_COLUMN_IDS : CONNECTION_COLUMN_IDS;
  const [order, setOrder] = useStoredColumnOrder(`relay.usage.${field}ColumnOrder.v2`, defaults);
  const moveColumn = (column: AggregateColumnId, target: AggregateColumnId, after: boolean) => setOrder((current) => reorderColumns(current, column, target, after));
  const moveColumnBy = (column: AggregateColumnId, offset: number) => setOrder((current) => shiftColumn(current, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<AggregateColumnId, { label: string; cell: (group: AggregateRow) => ReactNode }> = {
    name: { label: field === "model" ? t("common.model") : t("usage.poolMember"), cell: (group) => <span className="usage-aggregate-name" data-relay-tooltip={group.name}>{group.name}</span> },
    requests: { label: t("usage.requests"), cell: (group) => <CompactNumber value={group.requests} locale={i18n.language} /> },
    success: { label: t("common.success"), cell: (group) => `${Math.round(group.success / group.requests * 100)}%` },
    breakdown: { label: t("usage.tokens"), cell: (group) => <TokenBreakdown group={group} locale={i18n.language} /> },
    total: { label: t("usage.totalTokens"), cell: (group) => <CompactNumber value={group.tokens} locale={i18n.language} /> },
    speed: { label: t("usage.generationSpeedShort"), cell: (group) => <SpeedValue value={group.generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} /> },
    timing: { label: t("usage.timing"), cell: (group) => formatTiming(group.ttftCount ? Math.round(group.ttft / group.ttftCount) : null, Math.round(group.duration / group.requests), i18n.resolvedLanguage ?? i18n.language, t) },
    input: { label: t("usage.inputTokens"), cell: (group) => <CompactNumber value={group.inputTokens} locale={i18n.language} /> },
    output: { label: t("usage.outputTokens"), cell: (group) => <CompactNumber value={group.outputTokens} locale={i18n.language} /> },
    cache: { label: t("usage.cachedInputTokens"), cell: (group) => group.cachedInputSamples ? <CompactNumber value={group.cachedInputTokens} locale={i18n.language} /> : "—" },
    equivalent: { label: t("usage.value"), cell: (group) => formatUsageApiEquivalent(group.apiEquivalent, i18n.language) },
  };
  if (!aggregateRows.length) return <EmptyState title={t("usage.emptyTitle")} description={empty} />;
  return <div className="relay-table-wrap">
    <table className={`relay-table usage-aggregate-table usage-sortable-table ${field === "connection" ? "usage-connections-table" : "usage-models-table"}`}>
      <colgroup>{order.map((id) => <col key={id} data-column={id} />)}</colgroup>
      <thead><tr>{order.map((id) => <th key={id} data-column={id} data-dragging={drag?.column === id ? "true" : undefined} data-drop={drag?.target === id && drag.column !== id ? (drag.after ? "after" : "before") : undefined}>
        <button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bind(id)}><span>{columns[id].label}</span></button>
      </th>)}</tr></thead>
      <tbody>{aggregateRows.map((group) => <tr key={group.name}>{order.map((id) => <td key={id} data-column={id} data-label={columns[id].label}>{columns[id].cell(group)}</td>)}</tr>)}</tbody>
    </table>
  </div>;
}
