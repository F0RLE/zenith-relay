import { memo, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState, StatusIcon } from "../../components/Ui";
import { loadRequestTableLayout, reorderColumns, REQUEST_TABLE_LAYOUT_KEY, shiftColumn, useColumnDrag } from "./useColumnLayout";
import type { RequestColumnId, RequestTableLayout } from "./useColumnLayout";
import { tokenSpeed } from "../../usageSpeed";
import { usageSpeedSample, type UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatDurationMs, formatReasoningEffort, formatReasoningSummary, formatTiming, requestStatusHint, requestStatusKind } from "./usageReportFormat";
import { CompactNumber, SpeedValue, RequestStatusGlyph } from "./usageReportParts";
import { UsageTableRow, type UsageColumns } from "./UsageTableRow";

type RequestsViewProps = {
  rows: UsageRow[];
  formatTime: (timestamp: string) => string;
  onSelect: (row: UsageRow) => void;
};

export const RequestsView = memo(function RequestsView({
  rows,
  formatTime,
  onSelect,
}: RequestsViewProps) {
  const { t } = useTranslation();
  return rows.length ? <RequestTable rows={rows} formatTime={formatTime} onSelect={onSelect} /> : <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
});

function RequestTable({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t, i18n } = useTranslation();
  const [layout, setLayout] = useState<RequestTableLayout>(loadRequestTableLayout);
  useEffect(() => {
    try { localStorage.setItem(REQUEST_TABLE_LAYOUT_KEY, JSON.stringify(layout)); } catch { }
  }, [layout]);

  const columns = useMemo<UsageColumns<RequestColumnId>>(() => ({
    time: { label: t("usage.time"), cell: (row) => <time dateTime={row.time}>{formatTime(row.time)}</time> },
    status: {
      label: t("common.status"),
      cell: (row) => {
        const status = requestStatusKind(row);
        const reason = requestStatusHint(row, t);
        return <span className="usage-request-status">
          <span className="usage-request-result" data-relay-tooltip={reason}>
            <StatusIcon
              status={status === "success" ? "ready" : status === "pending" || status === "cancelled" ? "info" : status}
              label={reason}
            ><RequestStatusGlyph row={row} /></StatusIcon>
          </span>
        </span>;
      },
    },
    model: { label: t("common.model"), cell: (row) => <code className="usage-request-model">{row.model ?? "-"}</code> },
    reasoning: { label: t("usage.reasoning"), cell: (row) => <span className="usage-request-reasoning" data-relay-tooltip={formatReasoningSummary(row, t)}>{formatReasoningEffort(row.effectiveReasoningEffort ?? row.requestedReasoningEffort, t)}</span> },
    connection: { label: t("usage.poolMember"), cell: (row) => <span className="usage-request-connection">{row.connection}</span> },
    timing: {
      label: t("usage.timing"),
      cell: (row) => {
        const value = row.imageUsage ? formatDurationMs(row.duration, i18n.resolvedLanguage ?? i18n.language, t) : formatTiming(row.ttft, row.duration, i18n.resolvedLanguage ?? i18n.language, t);
        return <span data-relay-tooltip={t(row.imageUsage ? "usage.responseTime" : "usage.elapsedTiming")}>{value.split(" / ").map((part, index) => <span key={index} className="usage-request-timing-part">{index ? " / " : ""}{part}</span>)}</span>;
      },
    },
    speed: { label: t("usage.generationSpeedShort"), cell: (row) => <SpeedValue value={row.imageUsage ? null : tokenSpeed(usageSpeedSample(row))} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} /> },
    tokens: { label: t("usage.tokens"), cell: (row) => row.tokens == null ? "-" : <CompactNumber value={row.tokens} locale={i18n.language} /> },
    equivalent: { label: t("usage.value"), cell: (row) => row.apiEquivalent ? formatUsageApiEquivalent(row.apiEquivalent, i18n.language) : "—" },
    request: {
      label: t("usage.openDetails"),
      cell: (row) => (
        <button
          type="button"
          className="request-link"
          aria-haspopup="dialog"
          aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`}
          data-relay-tooltip={row.requestId ?? undefined}
          onClick={() => onSelect(row)}
        >
          {t("usage.openDetails")}
          {row.attempt > 1 ? <small className="usage-request-sends">{t("usage.execution.send", { number: row.attempt })}</small> : null}
        </button>
      ),
    },
  }), [formatTime, i18n.language, i18n.resolvedLanguage, onSelect, t]);
  const moveColumn = (column: RequestColumnId, target: RequestColumnId, after = false) => setLayout((previousLayout) => ({ ...previousLayout, order: reorderColumns(previousLayout.order, column, target, after) }));
  const moveColumnBy = (column: RequestColumnId, offset: number) => setLayout((previousLayout) => ({ ...previousLayout, order: shiftColumn(previousLayout.order, column, offset) }));
  const { bind: bindColumnDrag, drag: columnDrag } = useColumnDrag(moveColumn, moveColumnBy);

  return <div className="relay-table-wrap usage-request-table-wrap">
    <table className="relay-table usage-request-table usage-sortable-table" style={{ gridTemplateColumns: layout.order.map((columnId) => columnId === "status" ? "min-content" : "auto").join(" ") }}>
      <thead><tr>{layout.order.map((columnId) => <th
        key={columnId}
        data-column={columnId}
        data-dragging={columnDrag?.column === columnId ? "true" : undefined}
        data-drop={columnDrag?.target === columnId && columnDrag.column !== columnId ? (columnDrag.after ? "after" : "before") : undefined}
      >
        <button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[columnId].label })} {...bindColumnDrag(columnId)}><span>{columns[columnId].label}</span></button>
      </th>)}</tr></thead>
      <tbody>{rows.map((row) => <UsageTableRow key={row.id} row={row} columns={columns} order={layout.order} />)}</tbody>
    </table>
  </div>;
}
