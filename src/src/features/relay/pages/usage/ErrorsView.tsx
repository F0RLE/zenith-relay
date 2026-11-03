import { memo, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/Ui";
import { ERROR_COLUMN_IDS, reorderColumns, shiftColumn, useColumnDrag, useStoredColumnOrder } from "./useColumnLayout";
import type { ErrorColumnId } from "./useColumnLayout";
import type { UsageRow } from "./usageData";
import { compactRequestId, formatErrorCategory, formatErrorOrigin } from "./usageReportFormat";
import { UsageModel } from "./usageReportParts";
import { UsageTableRow, type UsageColumns } from "./UsageTableRow";

export const ErrorsView = memo(function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [order, setOrder] = useStoredColumnOrder("relay.usage.errorColumnOrder", ERROR_COLUMN_IDS);
  const moveColumn = (column: ErrorColumnId, target: ErrorColumnId, after: boolean) => setOrder((previousOrder) => reorderColumns(previousOrder, column, target, after));
  const moveColumnBy = (column: ErrorColumnId, offset: number) => setOrder((previousOrder) => shiftColumn(previousOrder, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns = useMemo<UsageColumns<ErrorColumnId>>(() => ({
    time: { label: t("usage.time"), cell: (row) => <time dateTime={row.time}>{formatTime(row.time)}</time> },
    model: { label: t("common.model"), cell: (row) => <UsageModel row={row} /> },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    origin: { label: t("usage.errorOrigin"), cell: (row) => formatErrorOrigin(row.errorOrigin, t) },
    error: { label: t("usage.errorCategory"), cell: (row) => <span data-relay-tooltip={row.errorCategory ?? undefined}>{formatErrorCategory(row.errorCategory, t)}</span> },
    request: { label: t("usage.requestId"), cell: (row) => <button type="button" className="request-link" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`} data-relay-tooltip={row.requestId ?? undefined} onClick={() => onSelect(row)}><code>{compactRequestId(row.requestId)}</code>{row.attempt > 1 ? <small className="usage-request-sends">{t("usage.execution.send", { number: row.attempt })}</small> : null}</button> },
  }), [formatTime, onSelect, t]);
  if (!rows.length) return <EmptyState title={t("usage.noErrors")} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-error-table usage-sortable-table">
    <colgroup>{order.map((columnId) => <col key={columnId} data-column={columnId} />)}</colgroup>
    <thead><tr>{order.map((columnId) => <th key={columnId} data-column={columnId} data-dragging={drag?.column === columnId ? "true" : undefined} data-drop={drag?.target === columnId && drag.column !== columnId ? drag.after ? "after" : "before" : undefined}><button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[columnId].label })} {...bind(columnId)}><span>{columns[columnId].label}</span></button></th>)}</tr></thead>
    <tbody>{rows.map((row) => <UsageTableRow key={row.id} row={row} columns={columns} order={order} />)}</tbody>
  </table></div>;
});
