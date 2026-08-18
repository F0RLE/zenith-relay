import { type KeyboardEvent, type PointerEvent, type ReactNode, useEffect, useState } from "react";
import { Activity, CreditCard, SlidersHorizontal, TrendingUp, X } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { AccountSummary, CacheWriteTtl, DefaultServiceTier, ErrorOrigin, ReasoningEffort, RoutingDiagnostics, ToolUseDiagnostics, UsageGroup, UsageTotals } from "../../api/types";
import { buildAccountValueProjection } from "../../accountEconomics";
import { CopyButton, Dialog, EmptyState, IconButton, OptionMenu, StatusBadge, StatusIcon, Tabs } from "../../components/Ui";
import { formatAccountValueMicroUsd } from "../../poolFormatting";
import { formatDetailedRemainingTime, formatQuotaRemaining, formatWindowDuration } from "../../quotaFormatting";
import { formatTokenSpeed, tokenSpeed, type TokenSpeedSample } from "../../usageSpeed";
import { emptyUsageTotals, formatCompactNumber, formatFullNumber } from "../../usageTotals";
import {
  CONNECTION_COLUMN_IDS,
  ERROR_COLUMN_IDS,
  loadRequestTableLayout,
  MODEL_COLUMN_IDS,
  reorderColumns,
  REQUEST_COLUMN_IDS,
  REQUEST_COLUMN_MAX_WIDTH,
  REQUEST_COLUMN_MIN_WIDTH,
  REQUEST_TABLE_LAYOUT_KEY,
  shiftColumn,
  useColumnDrag,
  useStoredColumnOrder,
  type AggregateColumnId,
  type ErrorColumnId,
  type RequestColumnId,
  type RequestTableLayout,
} from "./useColumnLayout";

export type UsageRow = {
  id: string | number;
  time: string;
  success: boolean;
  model: string | null;
  requestedReasoningEffort: ReasoningEffort | null;
  effectiveReasoningEffort: ReasoningEffort | null;
  connection: string;
  wireApi: string | null;
  serviceTier: DefaultServiceTier | null;
  appliedServiceTier: DefaultServiceTier | null;
  ttft: number | null;
  generationMs: number | null;
  duration: number;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  cacheWriteTtl: Exclude<CacheWriteTtl, "provider"> | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  errorOrigin: ErrorOrigin | null;
  toolUse: ToolUseDiagnostics | null;
  routing: RoutingDiagnostics | null;
  accountId: string | null;
  candidateKind: "account" | "source";
  apiEquivalent: UsageTotals["apiEquivalent"] | null;
};

export function RequestsView({ rows, status, setStatus, modelQuery, modelOptions, setModelQuery, connectionQuery, poolMemberOptions, setConnectionQuery, wireApi, setWireApi, errorQuery, setErrorQuery, requestQuery, setRequestQuery, clearFilters, formatTime, onSelect }: { rows: UsageRow[]; status: string; setStatus: (value: string) => void; modelQuery: string; modelOptions: Array<{ value: string; label: string }>; setModelQuery: (value: string) => void; connectionQuery: string; poolMemberOptions: Array<{ value: string; label: string }>; setConnectionQuery: (value: string) => void; wireApi: string; setWireApi: (value: string) => void; errorQuery: string; setErrorQuery: (value: string) => void; requestQuery: string; setRequestQuery: (value: string) => void; clearFilters: () => void; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const secondaryCount = [wireApi, errorQuery, requestQuery].filter(Boolean).length;
  const hasFilters = status !== "all" || Boolean(modelQuery || connectionQuery || secondaryCount);
  const errorOptions = [{ value: "", label: t("usage.anyErrorCategory") }, ...Array.from(new Set([...rows.flatMap((row) => row.errorCategory ? [row.errorCategory] : []), ...(errorQuery ? [errorQuery] : [])])).sort().map((value) => ({ value, label: formatErrorCategory(value, t) }))];
  return <><div className="usage-filter-panel">
    <div className="usage-filters usage-filter-primary">
      <OptionMenu className="filter-option-menu" label={t("common.status")} value={status} onChange={setStatus} options={[{ value: "all", label: t("usage.anyStatus") }, { value: "success", label: t("common.success") }, { value: "failed", label: t("common.failed") }]} />
      <OptionMenu className="filter-option-menu" label={t("common.model")} value={modelQuery} onChange={setModelQuery} options={modelOptions} />
      <OptionMenu className="filter-option-menu" label={t("usage.poolMember")} value={connectionQuery} onChange={setConnectionQuery} options={poolMemberOptions} />
    </div>
    <div className="usage-filter-controls">{hasFilters ? <IconButton label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}<span className="usage-filter-toggle-wrap"><IconButton className="usage-filter-toggle" label={t("usage.moreFilters")} icon={<SlidersHorizontal aria-hidden />} aria-expanded={showMoreFilters} onClick={() => setShowMoreFilters((current) => !current)} />{secondaryCount ? <small>{secondaryCount}</small> : null}</span></div>
    {showMoreFilters ? <div className="usage-filters usage-filter-secondary">
      <OptionMenu className="filter-option-menu" label={t("usage.protocol")} value={wireApi} onChange={setWireApi} options={[{ value: "", label: t("usage.anyProtocol") }, { value: "responses", label: "Responses" }, { value: "messages", label: "Messages" }, { value: "chat_completions", label: "Chat Completions" }]} />
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
    time: { label: t("usage.time"), cell: (row) => formatTime(row.time) },
    status: { label: t("common.status"), cell: (row) => <StatusIcon status={row.success ? "ready" : "error"} label={row.success ? t("common.success") : t("common.failed")} /> },
    model: { label: t("common.model"), cell: (row) => <UsageModel row={row} /> },
    protocol: { label: t("usage.protocol"), cell: (row) => <code>{formatWireApi(row.wireApi, t)}</code> },
    tier: { label: t("usage.serviceTier"), cell: (row) => formatServiceTier(row, t) },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    timing: { label: t("usage.timing"), cell: (row) => formatTiming(row.ttft, row.duration, i18n.resolvedLanguage ?? i18n.language, t) },
    speed: { label: t("usage.generationSpeedShort"), cell: (row) => <SpeedValue value={tokenSpeed(rowSpeedSample(row))} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} /> },
    tokens: { label: t("usage.tokens"), cell: (row) => row.tokens == null ? "-" : <CompactNumber value={row.tokens} locale={i18n.language} /> },
    equivalent: { label: t("usage.value"), cell: (row) => row.apiEquivalent ? formatApiEquivalent(row.apiEquivalent, i18n.language) : "—" },
    request: { label: t("usage.requestId"), cell: (row) => <button type="button" className="request-link" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`} onClick={() => onSelect(row)}><code>{row.requestId ?? "-"}</code></button> },
  };
  const resized = REQUEST_COLUMN_IDS.every((id) => layout.widths[id] != null);
  const totalWidth = resized ? REQUEST_COLUMN_IDS.reduce((total, id) => total + (layout.widths[id] ?? 0), 0) : 0;
  const captureWidths = (table: HTMLTableElement) => Object.fromEntries(Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]")).map((cell) => {
    const id = cell.dataset.column as RequestColumnId;
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

  return <div className="relay-table-wrap"><table className="relay-table usage-request-table usage-sortable-table" data-resized={resized ? "true" : "false"}>
    <colgroup>{layout.order.map((id) => <col key={id} data-column={id} style={resized ? { width: `${(layout.widths[id] ?? 0) / totalWidth * 100}%` } : undefined} />)}</colgroup>
    <thead><tr>{layout.order.map((id) => <th key={id} data-column={id} data-dragging={columnDrag?.column === id ? "true" : undefined} data-drop={columnDrag?.target === id && columnDrag.column !== id ? columnDrag.after ? "after" : "before" : undefined}>
      <button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bindColumnDrag(id)}><span>{columns[id].label}</span></button>
      <span className="usage-column-resizer" role="separator" tabIndex={0} aria-orientation="vertical" aria-label={t("usage.resizeColumn", { column: columns[id].label })} aria-valuemin={REQUEST_COLUMN_MIN_WIDTH[id]} aria-valuemax={REQUEST_COLUMN_MAX_WIDTH} aria-valuenow={Math.round(layout.widths[id] ?? REQUEST_COLUMN_MIN_WIDTH[id])} onPointerDown={(event) => startResize(event, id)} onPointerMove={(event) => resizeColumn(event, id)} onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); setResize(null); }} onLostPointerCapture={() => setResize(null)} onDoubleClick={() => setLayout((current) => ({ ...current, widths: {} }))} onKeyDown={(event) => resizeColumnByKeyboard(event, id)} />
    </th>)}</tr></thead>
    <tbody>{rows.map((row) => <tr key={row.id}>{layout.order.map((id) => <td key={id} data-column={id} title={id === "model" ? row.model ?? undefined : id === "connection" ? row.connection : undefined}>{columns[id].cell(row)}</td>)}</tr>)}</tbody>
  </table></div>;
}

export function UsageMetric({ icon, label, value, detail, title, className }: { icon?: ReactNode; label: string; value: ReactNode; detail?: ReactNode; title?: string; className?: string }) {
  return <div className={className} title={title}>{icon}<div className="usage-metric-copy"><span>{label}</span><strong>{value}</strong>{detail ? <small>{detail}</small> : null}</div></div>;
}

function formatTiming(ttft: number | null, duration: number, locale: string, t: TFunction) {
  return `${formatDurationMs(ttft, locale, t)} / ${formatDurationMs(duration, locale, t)}`;
}

type AggregateRow = {
  name: string;
  requests: number;
  success: number;
  inputTokens: number;
  cachedInputTokens: number;
  cachedInputSamples: number;
  cacheWriteInputTokens: number;
  cacheWriteInputSamples: number;
  reasoningTokens: number;
  outputTokens: number;
  tokens: number;
  ttft: number;
  ttftCount: number;
  duration: number;
  generationSpeed: number | null;
  apiEquivalent: UsageTotals["apiEquivalent"];
};

export function AggregateView({ rows, groups, field, empty }: { rows: UsageRow[]; groups?: UsageGroup[]; field: "model" | "connection"; empty: string }) {
  const { t, i18n } = useTranslation();
  const aggregateRows = groups?.map(({ key, label, totals }) => aggregateRowFromTotals(label || key || t("common.unknown"), totals)) ?? aggregateRowsFromUsage(rows, field, t("common.unknown"));
  const defaults: readonly AggregateColumnId[] = field === "model" ? MODEL_COLUMN_IDS : CONNECTION_COLUMN_IDS;
  const [order, setOrder] = useStoredColumnOrder(`relay.usage.${field}ColumnOrder.v2`, defaults);
  const moveColumn = (column: AggregateColumnId, target: AggregateColumnId, after: boolean) => setOrder((current) => reorderColumns(current, column, target, after));
  const moveColumnBy = (column: AggregateColumnId, offset: number) => setOrder((current) => shiftColumn(current, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<AggregateColumnId, { label: string; cell: (group: AggregateRow) => ReactNode }> = {
    name: { label: field === "model" ? t("common.model") : t("usage.poolMember"), cell: (group) => <code>{group.name}</code> },
    requests: { label: t("usage.requests"), cell: (group) => <CompactNumber value={group.requests} locale={i18n.language} /> },
    success: { label: t("common.success"), cell: (group) => `${Math.round(group.success / group.requests * 100)}%` },
    breakdown: { label: t("usage.tokens"), cell: (group) => <div className="usage-token-breakdown"><span title={`${t("usage.inputTokens")}: ${formatFullNumber(group.inputTokens, i18n.language)}`}><small>{t("usage.inputShort")}</small>{formatCompactNumber(group.inputTokens, i18n.language)}</span><span title={`${t("usage.outputTokens")}: ${formatFullNumber(group.outputTokens, i18n.language)}`}><small>{t("usage.outputShort")}</small>{formatCompactNumber(group.outputTokens, i18n.language)}</span><span title={`${t("usage.cachedInputTokens")}: ${group.cachedInputSamples ? formatFullNumber(group.cachedInputTokens, i18n.language) : t("common.unknown")}`}><small>{t("usage.cachedShort")}</small>{group.cachedInputSamples ? formatCompactNumber(group.cachedInputTokens, i18n.language) : "—"}</span>{group.cacheWriteInputSamples ? <span title={`${t("usage.cacheWriteInputTokens")}: ${formatFullNumber(group.cacheWriteInputTokens, i18n.language)}`}><small>{t("usage.cacheWriteShort")}</small>{formatCompactNumber(group.cacheWriteInputTokens, i18n.language)}</span> : null}<span title={`${t("usage.reasoningTokens")}: ${formatFullNumber(group.reasoningTokens, i18n.language)}`}><small>{t("usage.reasoningShort")}</small>{formatCompactNumber(group.reasoningTokens, i18n.language)}</span></div> },
    total: { label: t("usage.totalTokens"), cell: (group) => <CompactNumber value={group.tokens} locale={i18n.language} /> },
    speed: { label: t("usage.generationSpeedShort"), cell: (group) => <SpeedValue value={group.generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} /> },
    timing: { label: t("usage.timing"), cell: (group) => formatTiming(group.ttftCount ? Math.round(group.ttft / group.ttftCount) : null, Math.round(group.duration / group.requests), i18n.resolvedLanguage ?? i18n.language, t) },
    input: { label: t("usage.inputTokens"), cell: (group) => <CompactNumber value={group.inputTokens} locale={i18n.language} /> },
    output: { label: t("usage.outputTokens"), cell: (group) => <CompactNumber value={group.outputTokens} locale={i18n.language} /> },
    cache: { label: t("usage.cachedInputTokens"), cell: (group) => group.cachedInputSamples ? <CompactNumber value={group.cachedInputTokens} locale={i18n.language} /> : "—" },
    equivalent: { label: t("usage.value"), cell: (group) => formatApiEquivalent(group.apiEquivalent, i18n.language) },
  };
  if (!aggregateRows.length) return <EmptyState title={t("usage.emptyTitle")} description={empty} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-aggregate-table usage-sortable-table">
    <colgroup>{order.map((id) => <col key={id} data-column={id} />)}</colgroup>
    <thead><tr>{order.map((id) => <th key={id} data-column={id} data-dragging={drag?.column === id ? "true" : undefined} data-drop={drag?.target === id && drag.column !== id ? drag.after ? "after" : "before" : undefined}><button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bind(id)}><span>{columns[id].label}</span></button></th>)}</tr></thead>
    <tbody>{aggregateRows.map((group) => <tr key={group.name}>{order.map((id) => <td key={id} data-column={id}>{columns[id].cell(group)}</td>)}</tr>)}</tbody>
  </table></div>;
}

export function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [order, setOrder] = useStoredColumnOrder("relay.usage.errorColumnOrder", ERROR_COLUMN_IDS);
  const moveColumn = (column: ErrorColumnId, target: ErrorColumnId, after: boolean) => setOrder((current) => reorderColumns(current, column, target, after));
  const moveColumnBy = (column: ErrorColumnId, offset: number) => setOrder((current) => shiftColumn(current, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<ErrorColumnId, { label: string; cell: (row: UsageRow) => ReactNode }> = {
    time: { label: t("usage.time"), cell: (row) => formatTime(row.time) },
    model: { label: t("common.model"), cell: (row) => <UsageModel row={row} /> },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    origin: { label: t("usage.errorOrigin"), cell: (row) => formatErrorOrigin(row.errorOrigin, t) },
    error: { label: t("usage.errorCategory"), cell: (row) => <span title={row.errorCategory ?? undefined}>{formatErrorCategory(row.errorCategory, t)}</span> },
    request: { label: t("usage.requestId"), cell: (row) => <button type="button" className="request-link" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`} onClick={() => onSelect(row)}><code>{row.requestId ?? "-"}</code></button> },
  };
  if (!rows.length) return <EmptyState title={t("usage.noErrors")} description={t("usage.noErrorsHint")} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-error-table usage-sortable-table">
    <colgroup>{order.map((id) => <col key={id} data-column={id} />)}</colgroup>
    <thead><tr>{order.map((id) => <th key={id} data-column={id} data-dragging={drag?.column === id ? "true" : undefined} data-drop={drag?.target === id && drag.column !== id ? drag.after ? "after" : "before" : undefined}><button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bind(id)}><span>{columns[id].label}</span></button></th>)}</tr></thead>
    <tbody>{rows.map((row) => <tr key={row.id}>{order.map((id) => <td key={id} data-column={id}>{columns[id].cell(row)}</td>)}</tr>)}</tbody>
  </table></div>;
}

export function RequestDetails({ row, onClose }: { row: UsageRow; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const [section, setSection] = useState<"overview" | "tokens" | "tools" | "route">("overview");
  const routing = row.routing;
  const toolUse = row.toolUse;
  const speed = rowSpeedSample(row);
  const generationSpeed = tokenSpeed(speed);
  const visibleOutputTokens = row.outputTokens == null ? null : Math.max(0, row.outputTokens - (row.reasoningTokens ?? 0));
  const toolWarning = Boolean(
    toolUse
      && toolUse.forwardedToolCount > 0
      && toolUse.toolCallCount === 0
      && toolUse.terminalOutput === "text",
  );
  const tabs = [
    { id: "overview", label: t("usage.requestSections.overview") },
    { id: "tokens", label: t("usage.requestSections.tokens") },
    ...(toolUse ? [{ id: "tools", label: t("usage.requestSections.tools") }] : []),
    ...(routing ? [{ id: "route", label: t("usage.requestSections.route") }] : []),
  ];
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} wide className="request-details-dialog" closeOnBackdrop>
    <div className="request-details-header">
      <div className="request-details-identity">
        <StatusBadge status={row.success ? "ready" : "error"} label={row.success ? t("common.success") : t("common.failed")} />
        <code title={row.model ?? undefined}>{row.model ?? "-"}</code>
      </div>
      <div className="request-details-id"><span>{t("usage.requestId")}</span><code title={row.requestId ?? undefined}>{row.requestId ?? "-"}</code>{row.requestId ? <CopyButton value={row.requestId} label={t("usage.copyRequestId")} /> : null}</div>
    </div>
    <div className="request-details-metrics">
      <RequestDetailMetric label={t("usage.firstResponse")} value={formatDurationMs(row.ttft, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.generationSpeed")} value={<SpeedValue value={generationSpeed} locale={i18n.resolvedLanguage ?? i18n.language} unit={t("usage.tokensPerSecondUnit")} />} />
      <RequestDetailMetric label={t("usage.totalTime")} value={formatDurationMs(row.duration, i18n.resolvedLanguage ?? i18n.language, t)} />
      <RequestDetailMetric label={t("usage.visibleOutputTokens")} value={visibleOutputTokens == null ? "—" : formatCompactNumber(visibleOutputTokens, i18n.language)} />
    </div>
    <Tabs value={section} items={tabs} onChange={(value) => setSection(value as typeof section)} label={t("usage.requestSectionsLabel")} />
    {section === "overview" ? <>
      <dl className="request-details-list">
        <div><dt>{t("usage.poolMember")}</dt><dd>{row.connection}</dd></div>
        <div><dt>{t("usage.protocol")}</dt><dd><code>{formatWireApi(row.wireApi, t)}</code></dd></div>
        <div><dt>{t("usage.serviceTier")}</dt><dd>{formatServiceTier(row, t, "-")}</dd></div>
        <div><dt>{t("usage.reasoning")}</dt><dd>{formatReasoningSummary(row, t)}</dd></div>
      </dl>
      {!row.success ? <section className="request-details-error" aria-label={t("usage.errorDetails")}>
        <h3>{t("usage.errorDetails")}</h3>
        <dl className="request-details-list">
          <div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div>
          <div><dt>{t("usage.errorOrigin")}</dt><dd>{formatErrorOrigin(row.errorOrigin, t)}</dd></div>
          <div><dt>{t("usage.errorCategory")}</dt><dd title={row.errorCategory ?? undefined}>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : "-"}</dd></div>
        </dl>
      </section> : null}
    </> : null}
    {section === "tokens" ? <dl className="request-details-list request-details-token-list">
      <div><dt>{t("usage.inputTokens")}</dt><dd>{row.inputTokens ?? "-"}</dd></div>
      <div><dt>{t("usage.outputTokens")}</dt><dd>{row.outputTokens ?? "-"}</dd></div>
      <div><dt>{t("usage.cachedInputTokens")}</dt><dd>{row.cachedInputTokens ?? "-"}</dd></div>
      {row.cacheWriteInputTokens != null ? <div><dt>{t("usage.cacheWriteInputTokens")}</dt><dd>{row.cacheWriteInputTokens}{row.cacheWriteTtl ? ` (${t(`usage.cacheWriteTtls.${row.cacheWriteTtl}`)})` : ""}</dd></div> : null}
      <div><dt>{t("usage.reasoningTokens")}</dt><dd>{row.reasoningTokens ?? "-"}</dd></div>
      <div><dt>{t("usage.totalTokens")}</dt><dd>{row.tokens ?? "-"}</dd></div>
      <div><dt>{t("usage.apiEquivalent")}</dt><dd title={row.apiEquivalent ? t("usage.requestApiEquivalentHint", { count: row.apiEquivalent.unpricedTokens }) : undefined}>{row.apiEquivalent ? formatApiEquivalent(row.apiEquivalent, i18n.language) : "—"}</dd></div>
    </dl> : null}
    {section === "tools" && toolUse ? <section className="request-details-section">
      <dl className="request-details-list">
        <div><dt>{t("usage.clientTools")}</dt><dd>{toolUse.clientToolCount} → {toolUse.forwardedToolCount}</dd></div>
        <div><dt>{t("usage.toolChoice")}</dt><dd>{formatToolChoice(toolUse.toolChoice, t)}</dd></div>
        <div><dt>{t("usage.toolCallsReturned")}</dt><dd>{toolUse.toolCallCount}</dd></div>
        <div><dt>{t("usage.terminalOutput")}</dt><dd>{formatTerminalOutput(toolUse.terminalOutput, t)}</dd></div>
      </dl>
      {toolWarning ? <p className="form-note warning-text">{t("usage.toolCallMissing", { count: toolUse.forwardedToolCount })}</p> : null}
      <p className="form-note">{t("usage.toolDiagnosticsHint")}</p>
    </section> : null}
    {section === "route" && routing ? <dl className="request-details-list">
      <div><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div>
      <div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div>
      {row.candidateKind === "account" ? <div><dt>{t("usage.quotaAtSelection")}</dt><dd>{routing.quotaRemainingBasisPoints == null ? t("common.unknown") : `${(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%`}</dd></div> : null}
      <div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div>
      <div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div>
    </dl> : null}
  </Dialog>;
}

function RequestDetailMetric({ label, value }: { label: string; value: ReactNode }) {
  return <div className="request-details-metric"><span>{label}</span><strong>{value}</strong></div>;
}

function SpeedValue({ value, locale, unit }: { value: number | null; locale: string; unit: string }) {
  const { t } = useTranslation();
  return <span className="usage-speed-value" title={t("usage.generationSpeedHint")}>{formatTokenSpeed(value, locale, unit)}</span>;
}

function formatDurationMs(value: number | null, locale: string, t: TFunction): string {
  if (value == null || !Number.isFinite(value)) return "—";
  if (value >= 1000) return t("usage.durationSeconds", { value: new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value / 1000) });
  return t("usage.durationMilliseconds", { value: Math.round(value) });
}

function UsageModel({ row }: { row: Pick<UsageRow, "model" | "requestedReasoningEffort" | "effectiveReasoningEffort"> }) {
  const { t } = useTranslation();
  const requested = row.requestedReasoningEffort;
  const effective = row.effectiveReasoningEffort;
  const effort = effective ?? requested;
  const changed = Boolean(requested && effective && requested !== effective);
  return <span className="usage-model-value">
    <code>{row.model ?? "-"}</code>
    {effort ? <small title={changed ? t("usage.reasoningEffortChanged", { requested: formatReasoningEffort(requested, t), effective: formatReasoningEffort(effective, t) }) : undefined}>{changed ? `${formatReasoningEffort(requested, t)} → ${formatReasoningEffort(effective, t)}` : formatReasoningEffort(effort, t)}</small> : null}
  </span>;
}

function formatReasoningEffort(effort: ReasoningEffort | null, t: TFunction): string {
  return effort ? t(`usage.reasoningEfforts.${effort}`) : "-";
}

function formatReasoningSummary(row: Pick<UsageRow, "requestedReasoningEffort" | "effectiveReasoningEffort">, t: TFunction): string {
  if (row.requestedReasoningEffort && row.effectiveReasoningEffort && row.requestedReasoningEffort !== row.effectiveReasoningEffort) {
    return t("usage.reasoningEffortChanged", { requested: formatReasoningEffort(row.requestedReasoningEffort, t), effective: formatReasoningEffort(row.effectiveReasoningEffort, t) });
  }
  return formatReasoningEffort(row.effectiveReasoningEffort ?? row.requestedReasoningEffort, t);
}

function formatServiceTier(row: Pick<UsageRow, "serviceTier" | "appliedServiceTier">, t: TFunction, fallback = "—") {
  if (!row.serviceTier) return fallback;
  const requested = t(`pool.serviceTiers.${row.serviceTier}`);
  if (!row.appliedServiceTier || row.appliedServiceTier === row.serviceTier) return requested;
  return `${requested} → ${t(`pool.serviceTiers.${row.appliedServiceTier}`)}`;
}

function formatWireApi(value: string | null, t: TFunction): string {
  if (value === "responses") return t("usage.protocols.responses");
  if (value === "messages") return t("usage.protocols.messages");
  if (value === "chat_completions") return t("usage.protocols.chatCompletions");
  return value ?? "—";
}

function formatErrorCategory(category: string | null, t: TFunction): string {
  if (!category) return t("common.unknown");
  return t(`usage.errorCategories.${category}`, { defaultValue: category.replace(/_/g, " ") });
}

function formatErrorOrigin(origin: ErrorOrigin | null, t: TFunction): string {
  return origin ? t(`usage.errorOrigins.${origin}`) : t("common.unknown");
}

function formatToolChoice(choice: ToolUseDiagnostics["toolChoice"], t: TFunction): string {
  return t(`usage.toolChoices.${choice}`);
}

function formatTerminalOutput(output: ToolUseDiagnostics["terminalOutput"], t: TFunction): string {
  return t(`usage.terminalOutputs.${output}`);
}

function rowSpeedSample(row: UsageRow): TokenSpeedSample {
  return { success: row.success, outputTokens: row.outputTokens, durationMs: row.generationMs };
}

export function totalsFromRows(rows: UsageRow[]): UsageTotals {
  return rows.reduce<UsageTotals>((totals, row) => {
    const outputTokens = row.success ? Math.max(0, row.outputTokens ?? 0) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(row.success);
    totals.latencyMs += row.duration;
    if (row.ttft != null) {
      totals.ttftMs += row.ttft;
      totals.ttftSamples += 1;
    }
    if (row.success && row.generationMs != null) {
      totals.generationMs += Math.max(0, row.generationMs);
      totals.generationSamples += 1;
      totals.generationOutputTokens += outputTokens;
    }
    totals.inputTokens += row.inputTokens ?? 0;
    if (row.cachedInputTokens != null) {
      totals.cachedInputTokens += row.cachedInputTokens;
      totals.cachedInputSamples += 1;
    }
    if (row.cacheWriteInputTokens != null) {
      totals.cacheWriteInputTokens = (totals.cacheWriteInputTokens ?? 0) + row.cacheWriteInputTokens;
      totals.cacheWriteInputSamples = (totals.cacheWriteInputSamples ?? 0) + 1;
    }
    totals.reasoningTokens += row.reasoningTokens ?? 0;
    totals.outputTokens += row.outputTokens ?? 0;
    totals.totalTokens += row.tokens ?? 0;
    if (outputTokens > 0 && row.duration > 0) {
      totals.speedOutputTokens += outputTokens;
      totals.speedDurationMs += row.duration;
    }
    if (row.apiEquivalent) {
      totals.apiEquivalent.microUsd += row.apiEquivalent.microUsd;
      totals.apiEquivalent.pricedTokens += row.apiEquivalent.pricedTokens;
      totals.apiEquivalent.unpricedTokens += row.apiEquivalent.unpricedTokens;
    } else {
      totals.apiEquivalent.unpricedTokens += row.tokens ?? 0;
    }
    return totals;
  }, emptyUsageTotals());
}

function aggregateRowsFromUsage(rows: UsageRow[], field: "model" | "connection", unknown: string): AggregateRow[] {
  const groups = rows.reduce((map, row) => {
    const key = row[field] || unknown;
    map.set(key, [...(map.get(key) ?? []), row]);
    return map;
  }, new Map<string, UsageRow[]>());
  return [...groups.entries()].map(([name, groupRows]) => aggregateRowFromTotals(name, totalsFromRows(groupRows)));
}

function aggregateRowFromTotals(name: string, totals: UsageTotals): AggregateRow {
  return {
    name,
    requests: totals.requests,
    success: totals.successfulRequests,
    inputTokens: totals.inputTokens,
    cachedInputTokens: totals.cachedInputTokens,
    cachedInputSamples: totals.cachedInputSamples,
    cacheWriteInputTokens: totals.cacheWriteInputTokens ?? 0,
    cacheWriteInputSamples: totals.cacheWriteInputSamples ?? 0,
    reasoningTokens: totals.reasoningTokens,
    outputTokens: totals.outputTokens,
    tokens: totals.totalTokens,
    ttft: totals.ttftMs,
    ttftCount: totals.ttftSamples,
    duration: totals.latencyMs,
    generationSpeed: totals.generationMs ? totals.generationOutputTokens * 1_000 / totals.generationMs : null,
    apiEquivalent: totals.apiEquivalent,
  };
}

export function CompactNumber({ value, locale }: { value: number; locale: string }) {
  return <span title={formatFullNumber(value, locale)}>{formatCompactNumber(value, locale)}</span>;
}

export function formatApiEquivalent(value: UsageTotals["apiEquivalent"], locale: string) {
  if (!value.pricedTokens && value.unpricedTokens) return "-";
  const amount = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  }).format(value.microUsd / 1_000_000);
  return `≈${amount}`;
}

export function AccountUsageSummary({ account, totals }: { account: AccountSummary; totals: UsageTotals }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const nowMs = Date.now();
  const { purchaseCostMicroUsd: purchaseCost, potential, payback, approximate } = buildAccountValueProjection(totals.apiEquivalent, account.quota, account.purchaseCostMicroUsd);
  const paybackTitle = purchaseCost == null
    ? t("accounts.accountValue.purchaseMissing")
    : t("accounts.accountValue.paybackHint", {
      used: formatAccountValueMicroUsd(totals.apiEquivalent.microUsd, locale),
      purchase: formatAccountValueMicroUsd(purchaseCost, locale),
    });
  const windows = (["primary", "secondary"] as const)
    .map((kind) => ({ kind, quota: account.quota[kind] }))
    .filter(({ quota }) => Boolean(quota));
  return <section className="usage-account-value" aria-label={t("usage.accountUsage", { account: account.label })}>
    <header><div><span>{t("usage.selectedAccount")}</span><strong>{account.label}</strong></div><details><summary>{t("usage.howCalculated")}</summary><p>{t("usage.calculationHint")}</p></details></header>
    <div className="usage-account-metrics">
      <UsageMetric icon={<CreditCard aria-hidden />} label={t("accounts.accountValue.used")} value={formatApiEquivalent(totals.apiEquivalent, locale)} title={t("accounts.accountValue.usedHint", { count: totals.apiEquivalent.unpricedTokens })} />
      <UsageMetric icon={<TrendingUp aria-hidden />} label={t("accounts.accountValue.potential")} value={potential == null ? "—" : formatAccountValueMicroUsd(potential.microUsd, locale, potential.approximate)} title={t("accounts.accountValue.potentialHint")} />
      <UsageMetric icon={<CreditCard aria-hidden />} label={t("accounts.accountValue.purchaseCost")} value={purchaseCost == null ? "—" : formatAccountValueMicroUsd(purchaseCost, locale)} detail={purchaseCost == null ? t("accounts.accountValue.purchaseMissing") : undefined} />
      <UsageMetric icon={<Activity aria-hidden />} label={t("accounts.accountValue.payback")} value={payback == null ? "—" : `${approximate ? "≈" : ""}${new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }).format(payback)}`} title={paybackTitle} />
    </div>
    {windows.length ? <div className="relay-table-wrap"><table className="relay-table usage-window-table"><thead><tr><th>{t("usage.window")}</th><th>{t("usage.remaining")}</th><th>{t("usage.reset")}</th></tr></thead><tbody>{windows.map(({ kind, quota }) => <tr key={kind}><th scope="row">{formatWindowDuration(quota?.windowMinutes ?? null, locale, t("usage.window"))}</th><td>{formatQuotaRemaining(quota?.availableBasisPoints ?? null, locale)}</td><td>{quota?.resetAtMs == null ? "—" : formatDetailedRemainingTime(quota.resetAtMs, nowMs, t)}</td></tr>)}</tbody></table></div> : null}
  </section>;
}
