import { type KeyboardEvent, type PointerEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Activity, CalendarDays, CheckCircle2, ChevronLeft, ChevronRight, CreditCard, Database, Download, RefreshCw, SlidersHorizontal, Trash2, X } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RemoteUsageQuery, RoutingDiagnostics, UsageGroup, UsageTotals } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, OptionMenu, PageHeader, StatusIcon, Tabs, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { effectiveTokenSpeed, formatTokenSpeed, generationTokenSpeed, tokenSpeed, type TokenSpeedSample } from "../../usageSpeed";

type View = "requests" | "models" | "connections" | "errors";
type Range = "all" | "daily" | "weekly" | "monthly";
type UsageRow = {
  id: string | number;
  time: string;
  success: boolean;
  model: string | null;
  connection: string;
  key: string;
  wireApi: string | null;
  ttft: number | null;
  duration: number;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  routing: RoutingDiagnostics | null;
  accountId: string | null;
  generationDurationMs: number | null;
};

const REQUEST_COLUMN_IDS = ["time", "status", "model", "connection", "timing", "speed", "tokens", "request"] as const;
type RequestColumnId = typeof REQUEST_COLUMN_IDS[number];
type RequestTableLayout = { order: RequestColumnId[]; widths: Partial<Record<RequestColumnId, number>> };
const AGGREGATE_COLUMN_IDS = ["name", "requests", "success", "breakdown", "total", "speed", "timing"] as const;
type AggregateColumnId = typeof AGGREGATE_COLUMN_IDS[number];
const ERROR_COLUMN_IDS = ["time", "model", "connection", "error", "request"] as const;
type ErrorColumnId = typeof ERROR_COLUMN_IDS[number];
type ColumnDrag<ColumnId extends string> = { column: ColumnId; pointerId: number; target: ColumnId; after: boolean };
const REQUEST_TABLE_LAYOUT_KEY = "relay.usageRequestTableLayout";
const REQUEST_COLUMN_MAX_WIDTH = 480;
const REQUEST_COLUMN_MIN_WIDTH: Record<RequestColumnId, number> = { time: 130, status: 58, model: 82, connection: 100, timing: 96, speed: 82, tokens: 72, request: 120 };

function loadRequestTableLayout(): RequestTableLayout {
  const fallback = { order: [...REQUEST_COLUMN_IDS], widths: {} } satisfies RequestTableLayout;
  try {
    const parsed = JSON.parse(localStorage.getItem(REQUEST_TABLE_LAYOUT_KEY) ?? "null") as { order?: unknown; widths?: Record<string, unknown> } | null;
    const order = parsed?.order;
    if (!Array.isArray(order) || order.length !== REQUEST_COLUMN_IDS.length || new Set(order).size !== REQUEST_COLUMN_IDS.length || !order.every((id) => REQUEST_COLUMN_IDS.includes(id as RequestColumnId))) return fallback;
    const widths: Partial<Record<RequestColumnId, number>> = {};
    for (const id of REQUEST_COLUMN_IDS) {
      const width = Number(parsed?.widths?.[id]);
      if (Number.isFinite(width)) widths[id] = Math.min(REQUEST_COLUMN_MAX_WIDTH, Math.max(REQUEST_COLUMN_MIN_WIDTH[id], Math.round(width)));
    }
    return { order: order as RequestColumnId[], widths: Object.keys(widths).length === REQUEST_COLUMN_IDS.length ? widths : {} };
  } catch {
    return fallback;
  }
}

function reorderColumns<ColumnId extends string>(order: ColumnId[], column: ColumnId, target: ColumnId, after = false) {
  if (column === target) return order;
  const next = order.filter((id) => id !== column);
  next.splice(next.indexOf(target) + Number(after), 0, column);
  return next;
}

function shiftColumn<ColumnId extends string>(order: ColumnId[], column: ColumnId, offset: number) {
  const from = order.indexOf(column);
  const to = Math.min(order.length - 1, Math.max(0, from + offset));
  if (from === to) return order;
  const next = [...order];
  next.splice(to, 0, next.splice(from, 1)[0]);
  return next;
}

function useStoredColumnOrder<ColumnId extends string>(storageKey: string, defaults: readonly ColumnId[]) {
  const [order, setOrder] = useState<ColumnId[]>(() => {
    try {
      const stored = JSON.parse(localStorage.getItem(storageKey) ?? "null") as unknown;
      if (Array.isArray(stored) && stored.length === defaults.length && new Set(stored).size === defaults.length && stored.every((id) => defaults.includes(id as ColumnId))) return stored as ColumnId[];
    } catch { }
    return [...defaults];
  });
  useEffect(() => {
    try { localStorage.setItem(storageKey, JSON.stringify(order)); } catch { }
  }, [order, storageKey]);
  return [order, setOrder] as const;
}

function useColumnDrag<ColumnId extends string>(moveColumn: (column: ColumnId, target: ColumnId, after: boolean) => void, moveColumnBy: (column: ColumnId, offset: number) => void) {
  const [drag, setDrag] = useState<ColumnDrag<ColumnId> | null>(null);
  const dragRef = useRef<ColumnDrag<ColumnId> | null>(null);
  const cancel = () => { dragRef.current = null; setDrag(null); };
  const bind = (column: ColumnId) => ({
    onPointerDown: (event: PointerEvent<HTMLButtonElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      const next = { column, pointerId: event.pointerId, target: column, after: false };
      dragRef.current = next;
      setDrag(next);
    },
    onPointerMove: (event: PointerEvent<HTMLButtonElement>) => {
      const current = dragRef.current;
      const table = event.currentTarget.closest("table");
      if (!current || current.pointerId !== event.pointerId || !(table instanceof HTMLTableElement)) return;
      const headers = Array.from(table.querySelectorAll<HTMLTableCellElement>("thead th[data-column]"));
      const target = headers.find((header) => event.clientX <= header.getBoundingClientRect().right) ?? headers[headers.length - 1];
      if (!target) return;
      const bounds = target.getBoundingClientRect();
      const targetId = target.dataset.column as ColumnId;
      const after = event.clientX > bounds.left + bounds.width / 2;
      if (current.target === targetId && current.after === after) return;
      const next = { ...current, target: targetId, after };
      dragRef.current = next;
      setDrag(next);
    },
    onPointerUp: (event: PointerEvent<HTMLButtonElement>) => {
      const current = dragRef.current;
      if (!current || current.pointerId !== event.pointerId) return;
      if (current.column !== current.target) moveColumn(current.column, current.target, current.after);
      cancel();
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    },
    onPointerCancel: cancel,
    onLostPointerCapture: () => { if (dragRef.current?.column === column) cancel(); },
    onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => {
      if (!event.altKey || (event.key !== "ArrowLeft" && event.key !== "ArrowRight")) return;
      event.preventDefault();
      moveColumnBy(column, event.key === "ArrowLeft" ? -1 : 1);
    },
  });
  return { bind, drag };
}

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyUsage, refresh, loading, busy, perform, accountDisplayName } = useRelayState();
  const confirm = useConfirm();
  const [view, setView] = useState<View>("requests");
  const [status, setStatus] = useState("all");
  const [range, setRange] = useState<Range>("weekly");
  const [modelQuery, setModelQuery] = useState("");
  const [connectionQuery, setConnectionQuery] = useState("");
  const [keyQuery, setKeyQuery] = useState("");
  const [wireApi, setWireApi] = useState("");
  const [errorQuery, setErrorQuery] = useState("");
  const [requestQuery, setRequestQuery] = useState("");
  const [page, setPage] = useState(1);
  const [usageLoading, setUsageLoading] = useState(false);
  const [usageError, setUsageError] = useState(false);
  const [selected, setSelected] = useState<UsageRow | null>(null);
  const remoteUsageSupported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const requestFiltersActive = view === "requests";
  const usageQuery = useMemo<RemoteUsageQuery>(() => ({
    page,
    pageSize: 50,
    range: range === "all" ? undefined : range,
    modelQuery: requestFiltersActive ? modelQuery.trim() || undefined : undefined,
    sourceOrAccountQuery: requestFiltersActive ? connectionQuery.trim() || undefined : undefined,
    localKeyQuery: requestFiltersActive ? keyQuery.trim() || undefined : undefined,
    wireApi: requestFiltersActive && wireApi ? wireApi as RemoteUsageQuery["wireApi"] : undefined,
    success: view === "errors" ? false : requestFiltersActive && status !== "all" ? status === "success" : undefined,
    errorCategory: requestFiltersActive ? errorQuery.trim() || undefined : undefined,
    requestIdQuery: requestFiltersActive ? requestQuery.trim() || undefined : undefined,
  }), [page, range, modelQuery, connectionQuery, keyQuery, wireApi, status, errorQuery, requestQuery, view]);

  useEffect(() => {
    if (mode === "zenith" || !remoteUsageSupported) {
      setUsageLoading(false);
      return;
    }
    let active = true;
    setUsageLoading(true);
    setUsageError(false);
    const timer = window.setTimeout(() => {
      const load = mode === "local" ? loadLocalUsage : loadRemoteUsage;
      load(usageQuery)
        .catch(() => active && setUsageError(true))
        .finally(() => active && setUsageLoading(false));
    }, 200);
    return () => { active = false; window.clearTimeout(timer); };
  }, [mode, remoteUsageSupported, usageQuery, loadLocalUsage, loadRemoteUsage]);

  useEffect(() => {
    setPage(1);
    setSelected(null);
  }, [mode]);

  const accountLabels = useMemo(() => new Map(runtime?.accounts.map((account) => [account.id, account.label]) ?? []), [runtime?.accounts]);
  const sourceLabels = useMemo(() => new Map(runtime?.sources.map((source) => [source.id, source.name]) ?? []), [runtime?.sources]);
  const rows = useMemo<UsageRow[]>(() => {
    if (mode === "zenith") return readyUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.status === "success", model: item.modelDisplay || item.model, connection: "Zenith API", key: "Zenith API", wireApi: null, ttft: item.timeToFirstByteMs ?? null, duration: item.streamDurationMs ?? item.timeToFirstByteMs ?? 0, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, cacheWriteInputTokens: null, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.status === "success" ? 200 : null, errorCategory: item.status === "success" ? null : item.status, routing: null, accountId: null, generationDurationMs: item.streamDurationMs ?? item.timeToFirstByteMs ?? null }));
    if (mode === "remote") return remoteUsage.map((item) => ({ id: item.id, time: new Date(item.createdAtMs).toISOString(), success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.candidateKind === "account" ? accountDisplayName(null, item.candidateLabel) ?? t("accounts.importUnknownAccount") : item.candidateLabel ?? t("common.unknown"), key: item.localKeyId, wireApi: item.wireApi, ttft: item.ttftMs ?? null, duration: item.latencyMs, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, cacheWriteInputTokens: item.cacheWriteInputTokens ?? null, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory, routing: item.routing ?? null, accountId: null, generationDurationMs: item.generationMs ?? null }));
    return (localUsagePage?.events ?? []).map((item) => ({ id: item.id, time: item.createdAt, success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.accountId ? accountLabels.get(item.accountId) ?? t("accounts.importUnknownAccount") : sourceLabels.get(item.sourceId) ?? t("common.unknown"), key: item.localKeyId, wireApi: item.wireApi, ttft: item.ttftMs, duration: item.latencyMs, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, cacheWriteInputTokens: item.cacheWriteInputTokens ?? null, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory, routing: item.routing ?? null, accountId: item.accountId ?? null, generationDurationMs: item.generationMs }));
  }, [mode, readyUsage, remoteUsage, localUsagePage?.events, accountLabels, sourceLabels, accountDisplayName, t]);
  const cutoff = range === "all" ? 0 : Date.now() - (range === "daily" ? 1 : range === "weekly" ? 7 : 30) * 24 * 60 * 60 * 1_000;
  const filtered = mode !== "zenith" ? rows : rows.filter((item) => {
    if (new Date(item.time).getTime() < cutoff) return false;
    if (view === "errors") return !item.success;
    if (!requestFiltersActive) return true;
    return (status === "all" || (status === "success" ? item.success : !item.success))
      && (!requestQuery.trim() || item.requestId?.toLocaleLowerCase().includes(requestQuery.trim().toLocaleLowerCase()))
      && (!modelQuery.trim() || item.model?.toLocaleLowerCase().includes(modelQuery.trim().toLocaleLowerCase()))
      && (!connectionQuery.trim() || item.connection.toLocaleLowerCase().includes(connectionQuery.trim().toLocaleLowerCase()))
      && (!keyQuery.trim() || item.key.toLocaleLowerCase().includes(keyQuery.trim().toLocaleLowerCase()))
      && (!wireApi || item.wireApi === wireApi)
      && (!errorQuery.trim() || item.errorCategory === errorQuery.trim());
  });
  const usagePage = mode === "local" ? localUsagePage : mode === "remote" ? remoteUsagePage : null;
  const totals = usagePage?.totals ?? totalsFromRows(filtered);
  const averageFirstResponse = totals.ttftSamples ? Math.round(totals.ttftMs / totals.ttftSamples) : null;
  const averageDuration = totals.requests ? Math.round(totals.latencyMs / totals.requests) : null;
  const averageGenerationSpeed = totals.generationMs ? totals.generationOutputTokens * 1_000 / totals.generationMs : null;
  const averageEffectiveSpeed = totals.speedDurationMs ? totals.speedOutputTokens * 1_000 / totals.speedDurationMs : null;
  const successRate = totals.requests ? Math.round(totals.successfulRequests / totals.requests * 100) : null;
  const speedUnit = t("usage.tokensPerSecondUnit");
  const formatTime = (value: string) => new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "medium" }).format(new Date(value));
  const resetPage = (work: () => void) => { work(); setPage(1); setSelected(null); };
  const exportRows = () => perform("usage-export", () => relayCommands.exportUsage(filtered.map((row) => ({ time: row.time, success: row.success, model: row.model, connection: row.connection, latencyMs: row.duration, ttftMs: row.ttft, inputTokens: row.inputTokens, cachedInputTokens: row.cachedInputTokens, cacheWriteInputTokens: row.cacheWriteInputTokens, reasoningTokens: row.reasoningTokens, outputTokens: row.outputTokens, tokens: row.tokens, requestId: row.requestId, httpStatus: row.httpStatus, errorCategory: row.errorCategory }))), "feedback.exported");
  const reloadUsage = () => mode === "local" ? loadLocalUsage(usageQuery) : mode === "remote" ? loadRemoteUsage(usageQuery) : Promise.resolve();
  const clearLogs = async () => {
    if (!await confirm(t("usage.clearConfirm"), { danger: true })) return;
    setPage(1);
    if (await perform("usage-clear", () => mode === "local" ? relayCommands.clearLocalUsage() : relayCommands.remoteAction({ type: "clear_usage" }), "feedback.cleared")) await reloadUsage();
  };
  const canClear = mode === "local" || (mode === "remote" && remoteUsageSupported);
  const refreshUsage = async () => { await refresh(); await reloadUsage(); };
  const modelGroups = usagePage?.models;
  const poolMemberGroups = usagePage?.poolMembers?.map((group) => ({ ...group, label: mode === "remote" ? accountDisplayName(null, group.label) ?? group.label ?? t("common.unknown") : accountLabels.get(group.key) ?? sourceLabels.get(group.key) ?? group.label ?? t("common.unknown") }));
  const clearFilters = () => {
    setStatus("all"); setModelQuery(""); setConnectionQuery("");
    setKeyQuery(""); setWireApi(""); setErrorQuery(""); setRequestQuery("");
    setPage(1); setSelected(null);
  };

  if (mode === "remote" && !remoteUsageSupported) {
    return <section className="relay-page"><PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} /><EmptyState title={t("common.unsupported")} description={t("remote.capabilityUnavailable")} /></section>;
  }

  return <section className="relay-page">
    <PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} actions={<><ActionMenu className="usage-overflow"><ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={!canClear} title={!canClear ? t("usage.clearUnavailable") : undefined} onClick={clearLogs}>{t("usage.clearLogs")}</ActionMenuItem></ActionMenu><Button variant="primary" icon={<RefreshCw aria-hidden />} busy={loading || usageLoading} onClick={() => void refreshUsage()}>{t("common.refresh")}</Button></>} />
    <div className="usage-view-toolbar">
      <Tabs value={view} onChange={(id) => { setView(id as View); setPage(1); setSelected(null); }} label={t("usage.views")} items={[{ id: "requests", label: t("usage.requests") }, { id: "models", label: t("common.models") }, { id: "connections", label: t("usage.poolMembers") }, { id: "errors", label: t("overview.errors") }]} />
      <OptionMenu className="usage-range-menu" label={t("usage.range")} value={range} onChange={(value) => resetPage(() => setRange(value as Range))} icon={<CalendarDays aria-hidden />} options={[{ value: "daily", label: t("usage.daily") }, { value: "weekly", label: t("usage.weekly") }, { value: "monthly", label: t("usage.monthly") }, { value: "all", label: t("common.all") }]} />
    </div>
    <section className="usage-overview" aria-label={t("usage.summary")}>
      <div className="usage-metrics">
        <UsageMetric icon={<Activity aria-hidden />} label={t("usage.requests")} value={<CompactNumber value={totals.requests} locale={i18n.language} />} />
        <UsageMetric icon={<CheckCircle2 aria-hidden />} label={t("common.success")} value={successRate == null ? "-" : `${successRate}%`} detail={`${formatFullNumber(totals.successfulRequests, i18n.language)} / ${formatFullNumber(totals.requests, i18n.language)}`} />
        <UsageMetric icon={<Database aria-hidden />} label={t("usage.totalTokens")} value={<CompactNumber value={totals.totalTokens} locale={i18n.language} />} detail={`${t("usage.inputShort")} ${formatCompactNumber(totals.inputTokens, i18n.language)} · ${t("usage.cachedShort")} ${totals.cachedInputSamples ? formatCompactNumber(totals.cachedInputTokens, i18n.language) : "—"} · ${t("usage.outputShort")} ${formatCompactNumber(totals.outputTokens, i18n.language)}`} title={t("usage.tokenCompositionHint")} />
        <UsageMetric icon={<CreditCard aria-hidden />} label={t("usage.apiEquivalent")} value={formatApiEquivalent(totals.apiEquivalent, i18n.language)} detail={t("usage.pricedTokens", { count: formatCompactNumber(totals.apiEquivalent.pricedTokens, i18n.language) })} title={t("usage.apiEquivalentHint", { count: formatFullNumber(totals.apiEquivalent.unpricedTokens, i18n.language) })} />
      </div>
      <div className="usage-performance">
        <UsageMetric label={t("usage.firstResponse")} value={averageFirstResponse == null ? "-" : `${averageFirstResponse} ms`} />
        <UsageMetric label={t("usage.totalTime")} value={averageDuration == null ? "-" : `${averageDuration} ms`} />
        <UsageMetric label={t("usage.generationSpeed")} value={formatTokenSpeed(averageGenerationSpeed, i18n.resolvedLanguage ?? i18n.language, speedUnit)} title={t("usage.visibleSpeedHint")} />
        <UsageMetric label={t("usage.effectiveSpeed")} value={formatTokenSpeed(averageEffectiveSpeed, i18n.resolvedLanguage ?? i18n.language, speedUnit)} />
      </div>
    </section>
    <div className="usage-data-toolbar"><Button variant="secondary" icon={<Download aria-hidden />} busy={busy === "usage-export"} disabled={usageLoading} onClick={exportRows}>{t("common.export")}</Button></div>
    {view === "requests" ? <RequestsView rows={filtered} status={status} setStatus={(value) => resetPage(() => setStatus(value))} modelQuery={modelQuery} setModelQuery={(value) => resetPage(() => setModelQuery(value))} connectionQuery={connectionQuery} setConnectionQuery={(value) => resetPage(() => setConnectionQuery(value))} keyQuery={keyQuery} setKeyQuery={(value) => resetPage(() => setKeyQuery(value))} wireApi={wireApi} setWireApi={(value) => resetPage(() => setWireApi(value))} errorQuery={errorQuery} setErrorQuery={(value) => resetPage(() => setErrorQuery(value))} requestQuery={requestQuery} setRequestQuery={(value) => resetPage(() => setRequestQuery(value))} clearFilters={clearFilters} formatTime={formatTime} onSelect={setSelected} /> : null}
    {view === "models" ? <AggregateView rows={filtered} groups={modelGroups} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={filtered} groups={poolMemberGroups} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={filtered.filter((item) => !item.success)} formatTime={formatTime} onSelect={setSelected} /> : null}
    {usageError ? <p role="alert" className="form-note error-text">{t("usage.remoteLoadFailed")}</p> : null}
    {(view === "requests" || view === "errors") && usagePage && usagePage.totalPages > 1 ? <nav className="usage-pagination" aria-label={t("usage.pagination")}><Button variant="secondary" icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || usageLoading} onClick={() => setPage((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><span>{t("usage.page", { page: usagePage.page, total: usagePage.totalPages })}</span><Button variant="secondary" icon={<ChevronRight aria-hidden />} disabled={page >= usagePage.totalPages || usageLoading} onClick={() => setPage((value) => value + 1)}>{t("common.continue")}</Button></nav> : null}
    {selected ? <RequestDetails row={selected} onClose={() => setSelected(null)} /> : null}
  </section>;
}

function RequestsView({ rows, status, setStatus, modelQuery, setModelQuery, connectionQuery, setConnectionQuery, keyQuery, setKeyQuery, wireApi, setWireApi, errorQuery, setErrorQuery, requestQuery, setRequestQuery, clearFilters, formatTime, onSelect }: { rows: UsageRow[]; status: string; setStatus: (value: string) => void; modelQuery: string; setModelQuery: (value: string) => void; connectionQuery: string; setConnectionQuery: (value: string) => void; keyQuery: string; setKeyQuery: (value: string) => void; wireApi: string; setWireApi: (value: string) => void; errorQuery: string; setErrorQuery: (value: string) => void; requestQuery: string; setRequestQuery: (value: string) => void; clearFilters: () => void; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const secondaryCount = [keyQuery, wireApi, errorQuery, requestQuery].filter(Boolean).length;
  const hasFilters = status !== "all" || Boolean(modelQuery || connectionQuery || secondaryCount);
  return <><div className="usage-filter-panel">
    <div className="usage-filters usage-filter-primary">
      <OptionMenu className="filter-option-menu" label={t("common.status")} value={status} onChange={setStatus} options={[{ value: "all", label: t("usage.anyStatus") }, { value: "success", label: t("common.success") }, { value: "failed", label: t("common.failed") }]} />
      <input value={modelQuery} onChange={(event) => setModelQuery(event.target.value)} aria-label={t("common.model")} placeholder={t("common.model")} />
      <input value={connectionQuery} onChange={(event) => setConnectionQuery(event.target.value)} aria-label={t("usage.poolMember")} placeholder={t("usage.poolMember")} />
    </div>
    <div className="usage-filter-controls">{hasFilters ? <IconButton label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}<span className="usage-filter-toggle-wrap"><IconButton className="usage-filter-toggle" label={t("usage.moreFilters")} icon={<SlidersHorizontal aria-hidden />} aria-expanded={showMoreFilters} onClick={() => setShowMoreFilters((current) => !current)} />{secondaryCount ? <small>{secondaryCount}</small> : null}</span></div>
    {showMoreFilters ? <div className="usage-filters usage-filter-secondary">
      <input value={keyQuery} onChange={(event) => setKeyQuery(event.target.value)} aria-label={t("usage.localKey")} placeholder={t("usage.localKey")} />
      <OptionMenu className="filter-option-menu" label={t("usage.protocol")} value={wireApi} onChange={setWireApi} options={[{ value: "", label: t("usage.anyProtocol") }, { value: "responses", label: "Responses" }, { value: "chat_completions", label: "Chat Completions" }]} />
      <input value={errorQuery} onChange={(event) => setErrorQuery(event.target.value)} aria-label={t("usage.errorCategory")} placeholder={t("usage.errorCategory")} />
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
    model: { label: t("common.model"), cell: (row) => <code>{row.model ?? "-"}</code> },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
    timing: { label: t("usage.timing"), cell: (row) => formatTiming(row.ttft, row.duration) },
    speed: { label: t("usage.speed"), cell: (row) => formatTokenSpeed(tokenSpeed(rowSpeedSample(row)), i18n.resolvedLanguage ?? i18n.language, t("usage.tokensPerSecondUnit")) },
    tokens: { label: t("usage.tokens"), cell: (row) => row.tokens == null ? "-" : <CompactNumber value={row.tokens} locale={i18n.language} /> },
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

function UsageMetric({ icon, label, value, detail, title }: { icon?: ReactNode; label: string; value: ReactNode; detail?: ReactNode; title?: string }) {
  return <div title={title}>{icon}<div className="usage-metric-copy"><span>{label}</span><strong>{value}</strong>{detail ? <small>{detail}</small> : null}</div></div>;
}

function formatTiming(ttft: number | null, duration: number) { return `${ttft ?? "-"} / ${duration} ms`; }

type AggregateRow = {
  name: string;
  requests: number;
  success: number;
  inputTokens: number;
  cachedInputTokens: number;
  cachedInputSamples: number;
  reasoningTokens: number;
  outputTokens: number;
  tokens: number;
  ttft: number;
  ttftCount: number;
  duration: number;
  speed: number | null;
  generationSpeed: number | null;
};

function AggregateView({ rows, groups, field, empty }: { rows: UsageRow[]; groups?: UsageGroup[]; field: "model" | "connection"; empty: string }) {
  const { t, i18n } = useTranslation();
  const aggregateRows = groups?.map(({ key, label, totals }) => aggregateRowFromTotals(label || key || t("common.unknown"), totals)) ?? aggregateRowsFromUsage(rows, field, t("common.unknown"));
  const [order, setOrder] = useStoredColumnOrder(`relay.usage.${field}ColumnOrder`, AGGREGATE_COLUMN_IDS);
  const moveColumn = (column: AggregateColumnId, target: AggregateColumnId, after: boolean) => setOrder((current) => reorderColumns(current, column, target, after));
  const moveColumnBy = (column: AggregateColumnId, offset: number) => setOrder((current) => shiftColumn(current, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<AggregateColumnId, { label: string; cell: (group: AggregateRow) => ReactNode }> = {
    name: { label: field === "model" ? t("common.model") : t("usage.poolMember"), cell: (group) => <code>{group.name}</code> },
    requests: { label: t("usage.requests"), cell: (group) => <CompactNumber value={group.requests} locale={i18n.language} /> },
    success: { label: t("common.success"), cell: (group) => `${Math.round(group.success / group.requests * 100)}%` },
    breakdown: { label: t("usage.tokens"), cell: (group) => <div className="usage-token-breakdown"><span title={`${t("usage.inputTokens")}: ${formatFullNumber(group.inputTokens, i18n.language)}`}><small>{t("usage.inputShort")}</small>{formatCompactNumber(group.inputTokens, i18n.language)}</span><span title={`${t("usage.cachedInputTokens")}: ${group.cachedInputSamples ? formatFullNumber(group.cachedInputTokens, i18n.language) : t("common.unknown")}`}><small>{t("usage.cachedShort")}</small>{group.cachedInputSamples ? formatCompactNumber(group.cachedInputTokens, i18n.language) : "—"}</span><span title={`${t("usage.reasoningTokens")}: ${formatFullNumber(group.reasoningTokens, i18n.language)}`}><small>{t("usage.reasoningShort")}</small>{formatCompactNumber(group.reasoningTokens, i18n.language)}</span><span title={`${t("usage.outputTokens")}: ${formatFullNumber(group.outputTokens, i18n.language)}`}><small>{t("usage.outputShort")}</small>{formatCompactNumber(group.outputTokens, i18n.language)}</span></div> },
    total: { label: t("usage.totalTokens"), cell: (group) => <CompactNumber value={group.tokens} locale={i18n.language} /> },
    speed: { label: t("usage.generationSpeed"), cell: (group) => formatTokenSpeed(group.generationSpeed, i18n.resolvedLanguage ?? i18n.language, t("usage.tokensPerSecondUnit")) },
    timing: { label: t("usage.timing"), cell: (group) => formatTiming(group.ttftCount ? Math.round(group.ttft / group.ttftCount) : null, Math.round(group.duration / group.requests)) },
  };
  if (!aggregateRows.length) return <EmptyState title={t("usage.emptyTitle")} description={empty} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-aggregate-table usage-sortable-table">
    <colgroup>{order.map((id) => <col key={id} data-column={id} />)}</colgroup>
    <thead><tr>{order.map((id) => <th key={id} data-column={id} data-dragging={drag?.column === id ? "true" : undefined} data-drop={drag?.target === id && drag.column !== id ? drag.after ? "after" : "before" : undefined}><button type="button" className="usage-column-heading" aria-label={t("usage.moveColumn", { column: columns[id].label })} {...bind(id)}><span>{columns[id].label}</span></button></th>)}</tr></thead>
    <tbody>{aggregateRows.map((group) => <tr key={group.name}>{order.map((id) => <td key={id} data-column={id}>{columns[id].cell(group)}</td>)}</tr>)}</tbody>
  </table></div>;
}

function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [order, setOrder] = useStoredColumnOrder("relay.usage.errorColumnOrder", ERROR_COLUMN_IDS);
  const moveColumn = (column: ErrorColumnId, target: ErrorColumnId, after: boolean) => setOrder((current) => reorderColumns(current, column, target, after));
  const moveColumnBy = (column: ErrorColumnId, offset: number) => setOrder((current) => shiftColumn(current, column, offset));
  const { bind, drag } = useColumnDrag(moveColumn, moveColumnBy);
  const columns: Record<ErrorColumnId, { label: string; cell: (row: UsageRow) => ReactNode }> = {
    time: { label: t("usage.time"), cell: (row) => formatTime(row.time) },
    model: { label: t("common.model"), cell: (row) => <code>{row.model ?? "-"}</code> },
    connection: { label: t("usage.poolMember"), cell: (row) => row.connection },
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

function RequestDetails({ row, onClose }: { row: UsageRow; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const routing = row.routing;
  const speed = rowSpeedSample(row);
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}><dl className="detail-list"><div><dt>{t("usage.requestId")}</dt><dd><code>{row.requestId ?? "-"}</code></dd></div><div><dt>{t("common.status")}</dt><dd>{row.success ? t("common.success") : t("common.failed")}</dd></div><div><dt>{t("common.model")}</dt><dd><code>{row.model ?? "-"}</code></dd></div><div><dt>{t("usage.poolMember")}</dt><dd>{row.connection}</dd></div><div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div><div><dt>{t("usage.errorCategory")}</dt><dd title={row.errorCategory ?? undefined}>{row.errorCategory ? formatErrorCategory(row.errorCategory, t) : "-"}</dd></div><div><dt>{t("usage.firstResponse")}</dt><dd>{row.ttft == null ? "-" : `${row.ttft} ms`}</dd></div><div><dt>{t("usage.generationTime")}</dt><dd>{row.generationDurationMs == null ? "-" : `${row.generationDurationMs} ms`}</dd></div><div><dt>{t("usage.totalTime")}</dt><dd>{row.duration} ms</dd></div><div><dt>{t("usage.generationSpeed")}</dt><dd>{formatTokenSpeed(generationTokenSpeed(speed), i18n.resolvedLanguage ?? i18n.language, t("usage.tokensPerSecondUnit"))}</dd></div><div><dt>{t("usage.effectiveSpeed")}</dt><dd>{formatTokenSpeed(effectiveTokenSpeed(speed), i18n.resolvedLanguage ?? i18n.language, t("usage.tokensPerSecondUnit"))}</dd></div><div><dt>{t("usage.inputTokens")}</dt><dd>{row.inputTokens ?? "-"}</dd></div><div><dt>{t("usage.cachedInputTokens")}</dt><dd>{row.cachedInputTokens ?? "-"}</dd></div><div><dt>{t("usage.reasoningTokens")}</dt><dd>{row.reasoningTokens ?? "-"}</dd></div><div><dt>{t("usage.outputTokens")}</dt><dd>{row.outputTokens ?? "-"}</dd></div><div><dt>{t("usage.totalTokens")}</dt><dd>{row.tokens ?? "-"}</dd></div>{routing ? <><div className="detail-section-heading"><dt>{t("usage.routingDiagnostics")}</dt><dd /></div><div><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div><div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div><div><dt>{t("usage.quotaAtSelection")}</dt><dd>{routing.quotaRemainingBasisPoints == null ? t("common.unknown") : `${(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%`}</dd></div><div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div><div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div></> : null}</dl><p className="form-note">{t("usage.redactionHint")}</p></Dialog>;
}

function formatErrorCategory(category: string | null, t: TFunction): string {
  if (!category) return t("common.unknown");
  return t(`usage.errorCategories.${category}`, { defaultValue: category.replace(/_/g, " ") });
}

function rowSpeedSample(row: UsageRow): TokenSpeedSample {
  return { success: row.success, outputTokens: row.outputTokens, reasoningTokens: row.reasoningTokens, durationMs: row.duration, ttftMs: row.ttft, generationDurationMs: row.generationDurationMs };
}

function totalsFromRows(rows: UsageRow[]): UsageTotals {
  return rows.reduce<UsageTotals>((totals, row) => {
    const visibleOutputTokens = row.success ? Math.max(0, (row.outputTokens ?? 0) - (row.reasoningTokens ?? 0)) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(row.success);
    totals.latencyMs += row.duration;
    if (row.ttft != null) {
      totals.ttftMs += row.ttft;
      totals.ttftSamples += 1;
    }
    if (row.success && row.generationDurationMs != null && row.generationDurationMs > 0) {
      totals.generationMs += row.generationDurationMs;
      totals.generationSamples += 1;
      totals.generationOutputTokens += visibleOutputTokens;
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
    if (visibleOutputTokens > 0 && row.duration > 0) {
      totals.speedOutputTokens += visibleOutputTokens;
      totals.speedDurationMs += row.duration;
    }
    totals.apiEquivalent.unpricedTokens += row.tokens ?? 0;
    return totals;
  }, emptyTotals());
}

function emptyTotals(): UsageTotals {
  return {
    requests: 0, successfulRequests: 0, latencyMs: 0, ttftMs: 0, ttftSamples: 0,
    generationMs: 0, generationSamples: 0, generationOutputTokens: 0,
    inputTokens: 0, cachedInputTokens: 0, cacheWriteInputTokens: 0, reasoningTokens: 0, outputTokens: 0,
    cachedInputSamples: 0, cacheWriteInputSamples: 0,
    totalTokens: 0, speedOutputTokens: 0, speedDurationMs: 0,
    apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 },
  };
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
    reasoningTokens: totals.reasoningTokens,
    outputTokens: totals.outputTokens,
    tokens: totals.totalTokens,
    ttft: totals.ttftMs,
    ttftCount: totals.ttftSamples,
    duration: totals.latencyMs,
    speed: totals.speedDurationMs ? totals.speedOutputTokens * 1_000 / totals.speedDurationMs : null,
    generationSpeed: totals.generationMs ? totals.generationOutputTokens * 1_000 / totals.generationMs : null,
  };
}

function CompactNumber({ value, locale }: { value: number; locale: string }) {
  return <span title={formatFullNumber(value, locale)}>{formatCompactNumber(value, locale)}</span>;
}

function formatCompactNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, {
    notation: Math.abs(value) >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(value);
}

function formatFullNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value);
}

function formatApiEquivalent(value: UsageTotals["apiEquivalent"], locale: string) {
  if (!value.pricedTokens && value.unpricedTokens) return "-";
  const amount = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  }).format(value.microUsd / 1_000_000);
  return `≈${amount}${value.unpricedTokens ? "*" : ""}`;
}
