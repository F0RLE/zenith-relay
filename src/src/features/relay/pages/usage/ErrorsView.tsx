import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/Ui";
import { ERROR_COLUMN_IDS, reorderColumns, shiftColumn, useColumnDrag, useStoredColumnOrder } from "./useColumnLayout";
import type { ErrorColumnId } from "./useColumnLayout";
import type { UsageRow } from "./usageData";
import { formatErrorCategory, formatErrorOrigin } from "./usageReportFormat";
import { UsageModel } from "./usageReportParts";

export function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [order, setOrder] = useStoredColumnOrder("relay.usage.errorColumnOrder", ERROR_COLUMN_IDS);
  const moveColumn = (column: ErrorColumnId, target: ErrorColumnId, after: boolean) => setOrder((previousOrder) => reorderColumns(previousOrder, column, target, after));
  const moveColumnBy = (column: ErrorColumnId, offset: number) => setOrder((previousOrder) => shiftColumn(previousOrder, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<ErrorColumnId, { label: string; cell: (row: UsageRow) => ReactNode }> = {
    time: { label: t("usage.time"), cell: (row) => <time dateTime={row.time}>{formatTime(row.time)}</time> },
    model: { label: t("common.model"), cell: (row) => <UsageModel row={row} /> },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    origin: { label: t("usage.errorOrigin"), cell: (row) => formatErrorOrigin(row.errorOrigin, t) },
    error: { label: t("usage.errorCategory"), cell: (row) => <span data-relay-tooltip={row.errorCategory ?? undefined}>{formatErrorCategory(row.errorCategory, t)}</span> },
    request: { label: t("usage.requestId"), cell: (row) => <button type="button" className="request-link" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`} onClick={() => onSelect(row)}><code>{row.requestId ?? "-"}</code></button> },
  };
  if (!rows.length) return <EmptyState title={t("usage.noErrors")} description={t("usage.noErrorsHint")} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-error-table usage-sortable-table">
    <colgroup>{order.map((columnId) => <col key={columnId} data-column={columnId} />)}</colgroup>
    <thead><tr>{order.map((columnId) => <th key={columnId} data-column={columnId} data-dragging={drag?.column === columnId ? "true" : undefined} data-drop={drag?.target === columnId && drag.column !== columnId ? drag.after ? "after" : "before" : undefined}><button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[columnId].label })} {...bind(columnId)}><span>{columns[columnId].label}</span></button></th>)}</tr></thead>
    <tbody>{rows.map((row) => <tr key={row.id}>{order.map((columnId) => <td key={columnId} data-column={columnId} data-label={columns[columnId].label}>{columns[columnId].cell(row)}</td>)}</tr>)}</tbody>
  </table></div>;
}
