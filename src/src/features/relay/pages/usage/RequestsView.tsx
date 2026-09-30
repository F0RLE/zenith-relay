import { useEffect, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import { SlidersHorizontal, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { EmptyState, IconButton, OptionMenu, StatusIcon } from "../../components/Ui";
import { tokenSpeed } from "../../usageSpeed";
import { loadRequestTableLayout, reorderColumns, REQUEST_COLUMN_IDS, REQUEST_COLUMN_MAX_WIDTH, REQUEST_COLUMN_MIN_WIDTH, REQUEST_TABLE_LAYOUT_KEY, shiftColumn, useColumnDrag } from "./useColumnLayout";
import type { RequestColumnId, RequestTableLayout } from "./useColumnLayout";
import { usageSpeedSample } from "./usageData";
import type { UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";
import { formatTiming, requestStatusLabel, formatServiceTier, formatWireApi, formatErrorCategory } from "./usageReportFormat";
import { SpeedValue, UsageModel, CompactNumber } from "./usageReportParts";

type RequestsViewProps = {
  rows: UsageRow[];
  status: string;
  setStatus: (value: string) => void;
  modelQuery: string;
  modelOptions: Array<{ value: string; label: string }>;
  setModelQuery: (value: string) => void;
  connectionQuery: string;
  poolMemberOptions: Array<{ value: string; label: string }>;
  setConnectionQuery: (value: string) => void;
  wireApi: string;
  setWireApi: (value: string) => void;
  errorQuery: string;
  setErrorQuery: (value: string) => void;
  requestQuery: string;
  setRequestQuery: (value: string) => void;
  clearFilters: () => void;
  formatTime: (value: string) => string;
  onSelect: (row: UsageRow) => void;
};

export function RequestsView({
  rows,
  status,
  setStatus,
  modelQuery,
  modelOptions,
  setModelQuery,
  connectionQuery,
  poolMemberOptions,
  setConnectionQuery,
  wireApi,
  setWireApi,
  errorQuery,
  setErrorQuery,
  requestQuery,
  setRequestQuery,
  clearFilters,
  formatTime,
  onSelect,
}: RequestsViewProps) {
  const { t } = useTranslation();
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const secondaryCount = [wireApi, errorQuery, requestQuery].filter(Boolean).length;
  const hasFilters = status !== "all" || Boolean(modelQuery || connectionQuery || secondaryCount);
  const errorOptions = [
    { value: "", label: t("usage.anyErrorCategory") },
    ...Array.from(new Set([
      ...rows.flatMap((row) => row.errorCategory ? [row.errorCategory] : []),
      ...(errorQuery ? [errorQuery] : []),
    ])).sort().map((value) => ({ value, label: formatErrorCategory(value, t) })),
  ];
  return <><div className="usage-filter-panel">
    <div className="usage-filters usage-filter-primary">
      <OptionMenu
        className="filter-option-menu"
        label={t("common.status")}
        value={status}
        onChange={setStatus}
        options={[
          { value: "all", label: t("usage.anyStatus") },
          { value: "success", label: t("common.success") },
          { value: "failed", label: t("common.failed") },
        ]}
      />
      <OptionMenu className="filter-option-menu" label={t("common.model")} value={modelQuery} onChange={setModelQuery} options={modelOptions} />
      <OptionMenu className="filter-option-menu" label={t("usage.poolMember")} value={connectionQuery} onChange={setConnectionQuery} options={poolMemberOptions} />
    </div>
    <div className="usage-filter-controls">
      {hasFilters ? <IconButton label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}
      <span className="usage-filter-toggle-wrap">
        <IconButton
          className="usage-filter-toggle"
          label={t("usage.moreFilters")}
          icon={<SlidersHorizontal aria-hidden />}
          aria-expanded={showMoreFilters}
          onClick={() => setShowMoreFilters((current) => !current)}
        />
        {secondaryCount ? <small>{secondaryCount}</small> : null}
      </span>
    </div>
    {showMoreFilters ? <div className="usage-filters usage-filter-secondary">
      <OptionMenu
        className="filter-option-menu"
        label={t("usage.protocol")}
        value={wireApi}
        onChange={setWireApi}
        options={[
          { value: "", label: t("usage.anyProtocol") },
          { value: "responses", label: "Responses" },
          { value: "messages", label: "Messages" },
          { value: "chat_completions", label: "Chat Completions" },
          { value: "gemini", label: "Gemini" },
        ]}
      />
      <OptionMenu className="filter-option-menu" label={t("usage.errorCategory")} value={errorQuery} onChange={setErrorQuery} options={errorOptions} />
      <input value={requestQuery} onChange={(event) => setRequestQuery(event.target.value)} aria-label={t("usage.requestId")} placeholder={t("usage.requestId")} />
    </div> : null}
  </div>{rows.length ? <RequestTable rows={rows} formatTime={formatTime} onSelect={onSelect} /> : <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />}</>;
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
  const resized = REQUEST_COLUMN_IDS.every((id) => layout.widths[id] != null);
  const totalWidth = resized ? REQUEST_COLUMN_IDS.reduce((total, id) => total + (layout.widths[id] ?? 0), 0) : 0;
  const captureWidths = (table: HTMLTableElement) => Object.fromEntries(Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]")).map((cell) => {
    const id = cell.dataset["column"] as RequestColumnId;
    return [id, Math.max(REQUEST_COLUMN_MIN_WIDTH[id], Math.round(cell.getBoundingClientRect().width))];
  })) as Record<RequestColumnId, number>;
  const moveColumn = (column: RequestColumnId, target: RequestColumnId, after = false) => setLayout((current) => ({ ...current, order: reorderColumns(current.order, column, target, after) }));
  const moveColumnBy = (column: RequestColumnId, offset: number) => setLayout((current) => ({ ...current, order: shiftColumn(current.order, column, offset) }));
  const { bind: bindColumnDrag, drag: columnDrag } = useColumnDrag(moveColumn, moveColumnBy);
  const startResize = (event: PointerEvent<HTMLSpanElement>, column: RequestColumnId) => {
    const table = event.currentTarget.closest("table");
    const header = event.currentTarget.closest("th");
    if (!(table instanceof HTMLTableElement) || !(header instanceof HTMLTableCellElement)) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setLayout((current) => ({ ...current, widths: captureWidths(table) }));
    setResize({ column, pointerId: event.pointerId, startX: event.clientX, startWidth: header.getBoundingClientRect().width });
  };
  const resizeColumn = (event: PointerEvent<HTMLSpanElement>, column: RequestColumnId) => {
    if (!resize || resize.column !== column || resize.pointerId !== event.pointerId) return;
    const width = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[column], Math.round(resize.startWidth + event.clientX - resize.startX)));
    setLayout((current) => current.widths[column] === width ? current : { ...current, widths: { ...current.widths, [column]: width } });
  };
  const resizeColumnByKeyboard = (event: KeyboardEvent<HTMLSpanElement>, column: RequestColumnId) => {
    if (event.key === "Home") {
      event.preventDefault();
      setLayout((current) => ({ ...current, widths: {} }));
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const table = event.currentTarget.closest("table");
    if (!(table instanceof HTMLTableElement)) return;
    event.preventDefault();
    const widths = captureWidths(table);
    widths[column] = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[column], widths[column] + (event.key === "ArrowRight" ? 12 : -12)));
    setLayout((current) => ({ ...current, widths }));
  };

  return <div className="relay-table-wrap">
    <table className="relay-table usage-request-table usage-sortable-table" data-resized={resized ? "true" : "false"}>
      <colgroup>{layout.order.map((id) => <col key={id} data-column={id} style={resized ? { width: `${(layout.widths[id] ?? 0) / totalWidth * 100}%` } : undefined} />)}</colgroup>
      <thead><tr>{layout.order.map((id) => <th
        key={id}
        data-column={id}
        data-dragging={columnDrag?.column === id ? "true" : undefined}
        data-drop={columnDrag?.target === id && columnDrag.column !== id ? (columnDrag.after ? "after" : "before") : undefined}
      >
        <button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bindColumnDrag(id)}><span>{columns[id].label}</span></button>
        <span
          className="usage-column-resizer"
          role="separator"
          tabIndex={0}
          aria-orientation="vertical"
          aria-label={t("usage.resizeColumn", { column: columns[id].label })}
          aria-valuemin={REQUEST_COLUMN_MIN_WIDTH[id]}
          aria-valuemax={REQUEST_COLUMN_MAX_WIDTH}
          aria-valuenow={Math.round(layout.widths[id] ?? REQUEST_COLUMN_MIN_WIDTH[id])}
          onPointerDown={(event) => startResize(event, id)}
          onPointerMove={(event) => resizeColumn(event, id)}
          onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); setResize(null); }}
          onLostPointerCapture={() => setResize(null)}
          onDoubleClick={() => setLayout((current) => ({ ...current, widths: {} }))}
          onKeyDown={(event) => resizeColumnByKeyboard(event, id)}
        />
      </th>)}</tr></thead>
      <tbody>{rows.map((row) => <tr key={row.id}>{layout.order.map((id) => <td
        key={id}
        data-column={id}
        data-label={columns[id].label}
        data-relay-tooltip={id === "model" ? row.model ?? undefined : id === "connection" ? row.connection : undefined}
      >{columns[id].cell(row)}</td>)}</tr>)}</tbody>
    </table>
  </div>;
}
