import { useEffect, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState, StatusIcon } from "../../components/Ui";
import { tokenSpeed } from "../../usageSpeed";
import { loadRequestTableLayout, reorderColumns, REQUEST_COLUMN_IDS, REQUEST_COLUMN_MAX_WIDTH, REQUEST_COLUMN_MIN_WIDTH, REQUEST_TABLE_LAYOUT_KEY, shiftColumn, useColumnDrag } from "./useColumnLayout";
import type { RequestColumnId, RequestTableLayout } from "./useColumnLayout";
import { usageSpeedSample } from "./usageData";
import type { UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatTiming, requestStatusLabel, formatServiceTier, formatWireApi } from "./usageReportFormat";
import { SpeedValue, UsageModel, CompactNumber } from "./usageReportParts";

type RequestsViewProps = {
  rows: UsageRow[];
  formatTime: (timestamp: string) => string;
  onSelect: (row: UsageRow) => void;
};

export function RequestsView({
  rows,
  formatTime,
  onSelect,
}: RequestsViewProps) {
  const { t } = useTranslation();
  return rows.length ? <RequestTable rows={rows} formatTime={formatTime} onSelect={onSelect} /> : <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
}

function RequestTable({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t, i18n } = useTranslation();
  const [layout, setLayout] = useState<RequestTableLayout>(loadRequestTableLayout);
  const [resize, setResize] = useState<{ column: RequestColumnId; pointerId: number; startX: number; startWidth: number } | null>(null);
  useEffect(() => {
    try { localStorage.setItem(REQUEST_TABLE_LAYOUT_KEY, JSON.stringify(layout)); } catch { }
  }, [layout]);

  const columns: Record<RequestColumnId, { label: string; cell: (row: UsageRow) => ReactNode }> = {
    time: { label: t("usage.time"), cell: (row) => <time dateTime={row.time}>{formatTime(row.time)}</time> },
    status: {
      label: t("common.status"),
      cell: (row) => (
        <StatusIcon
          status={row.requestOrigin?.startsWith("blocked_") ? "warning" : row.success ? "ready" : "error"}
          label={requestStatusLabel(row, t)}
        />
      ),
    },
    model: { label: t("common.model"), cell: (row) => <UsageModel row={row} /> },
    protocol: { label: t("usage.protocol"), cell: (row) => <code>{formatWireApi(row.wireApi, t)}</code> },
    tier: { label: t("usage.serviceTier"), cell: (row) => formatServiceTier(row, t) },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    timing: { label: t("usage.timing"), cell: (row) => formatTiming(row.ttft, row.duration, i18n.resolvedLanguage ?? i18n.language, t) },
    speed: {
      label: t("usage.generationSpeedShort"),
      cell: (row) => (
        <SpeedValue
          value={tokenSpeed(usageSpeedSample(row))}
          locale={i18n.resolvedLanguage ?? i18n.language}
          unit={t("usage.tokensPerSecondUnit")}
        />
      ),
    },
    tokens: { label: t("usage.tokens"), cell: (row) => row.tokens == null ? "-" : <CompactNumber value={row.tokens} locale={i18n.language} /> },
    equivalent: { label: t("usage.value"), cell: (row) => row.apiEquivalent ? formatUsageApiEquivalent(row.apiEquivalent, i18n.language) : "—" },
    request: {
      label: t("usage.requestId"),
      cell: (row) => (
        <button
          type="button"
          className="request-link"
          aria-haspopup="dialog"
          aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`}
          onClick={() => onSelect(row)}
        >
          <code>{row.requestId ?? "-"}</code>
        </button>
      ),
    },
  };
  const resized = REQUEST_COLUMN_IDS.every((columnId) => layout.widths[columnId] != null);
  const totalWidth = resized ? REQUEST_COLUMN_IDS.reduce((total, columnId) => total + (layout.widths[columnId] ?? 0), 0) : 0;
  const captureWidths = (table: HTMLTableElement) => Object.fromEntries(Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]")).map((cell) => {
    const columnId = cell.dataset["column"] as RequestColumnId;
    return [columnId, Math.max(REQUEST_COLUMN_MIN_WIDTH[columnId], Math.round(cell.getBoundingClientRect().width))];
  })) as Record<RequestColumnId, number>;
  const moveColumn = (column: RequestColumnId, target: RequestColumnId, after = false) => setLayout((previousLayout) => ({ ...previousLayout, order: reorderColumns(previousLayout.order, column, target, after) }));
  const moveColumnBy = (column: RequestColumnId, offset: number) => setLayout((previousLayout) => ({ ...previousLayout, order: shiftColumn(previousLayout.order, column, offset) }));
  const { bind: bindColumnDrag, drag: columnDrag } = useColumnDrag(moveColumn, moveColumnBy);
  const startResize = (event: PointerEvent<HTMLSpanElement>, column: RequestColumnId) => {
    const table = event.currentTarget.closest("table");
    const header = event.currentTarget.closest("th");
    if (!(table instanceof HTMLTableElement) || !(header instanceof HTMLTableCellElement)) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setLayout((previousLayout) => ({ ...previousLayout, widths: captureWidths(table) }));
    setResize({ column, pointerId: event.pointerId, startX: event.clientX, startWidth: header.getBoundingClientRect().width });
  };
  const resizeColumn = (event: PointerEvent<HTMLSpanElement>, column: RequestColumnId) => {
    if (!resize || resize.column !== column || resize.pointerId !== event.pointerId) return;
    const width = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[column], Math.round(resize.startWidth + event.clientX - resize.startX)));
    setLayout((previousLayout) => previousLayout.widths[column] === width ? previousLayout : { ...previousLayout, widths: { ...previousLayout.widths, [column]: width } });
  };
  const resizeColumnByKeyboard = (event: KeyboardEvent<HTMLSpanElement>, column: RequestColumnId) => {
    if (event.key === "Home") {
      event.preventDefault();
      setLayout((previousLayout) => ({ ...previousLayout, widths: {} }));
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const table = event.currentTarget.closest("table");
    if (!(table instanceof HTMLTableElement)) return;
    event.preventDefault();
    const widths = captureWidths(table);
    widths[column] = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[column], widths[column] + (event.key === "ArrowRight" ? 12 : -12)));
    setLayout((previousLayout) => ({ ...previousLayout, widths }));
  };

  return <div className="relay-table-wrap">
    <table className="relay-table usage-request-table usage-sortable-table" data-resized={resized ? "true" : "false"}>
      <colgroup>{layout.order.map((columnId) => <col key={columnId} data-column={columnId} style={resized ? { width: `${(layout.widths[columnId] ?? 0) / totalWidth * 100}%` } : undefined} />)}</colgroup>
      <thead><tr>{layout.order.map((columnId) => <th
        key={columnId}
        data-column={columnId}
        data-dragging={columnDrag?.column === columnId ? "true" : undefined}
        data-drop={columnDrag?.target === columnId && columnDrag.column !== columnId ? (columnDrag.after ? "after" : "before") : undefined}
      >
        <button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[columnId].label })} {...bindColumnDrag(columnId)}><span>{columns[columnId].label}</span></button>
        <span
          className="usage-column-resizer"
          role="separator"
          tabIndex={0}
          aria-orientation="vertical"
          aria-label={t("usage.resizeColumn", { column: columns[columnId].label })}
          aria-valuemin={REQUEST_COLUMN_MIN_WIDTH[columnId]}
          aria-valuemax={REQUEST_COLUMN_MAX_WIDTH}
          aria-valuenow={Math.round(layout.widths[columnId] ?? REQUEST_COLUMN_MIN_WIDTH[columnId])}
          onPointerDown={(event) => startResize(event, columnId)}
          onPointerMove={(event) => resizeColumn(event, columnId)}
          onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); setResize(null); }}
          onLostPointerCapture={() => setResize(null)}
          onDoubleClick={() => setLayout((previousLayout) => ({ ...previousLayout, widths: {} }))}
          onKeyDown={(event) => resizeColumnByKeyboard(event, columnId)}
        />
      </th>)}</tr></thead>
      <tbody>{rows.map((row) => <tr key={row.id}>{layout.order.map((columnId) => <td
        key={columnId}
        data-column={columnId}
        data-label={columns[columnId].label}
        data-relay-tooltip={columnId === "model" ? row.model ?? undefined : columnId === "connection" ? row.connection : undefined}
      >{columns[columnId].cell(row)}</td>)}</tr>)}</tbody>
    </table>
  </div>;
}
