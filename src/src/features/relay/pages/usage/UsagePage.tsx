import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, ChevronRight, Download, RefreshCw, SlidersHorizontal, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RemoteUsageQuery, RoutingDiagnostics } from "../../api/types";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, PageHeader, StatusBadge, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

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
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  routing: RoutingDiagnostics | null;
};

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, localUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyUsage, refresh, loading, busy, perform } = useRelayState();
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
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remoteError, setRemoteError] = useState(false);
  const [selected, setSelected] = useState<UsageRow | null>(null);
  const remoteUsageSupported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const remoteQuery = useMemo<RemoteUsageQuery>(() => ({
    page,
    pageSize: 50,
    range: range === "all" ? undefined : range,
    modelQuery: modelQuery.trim() || undefined,
    sourceOrAccountQuery: connectionQuery.trim() || undefined,
    localKeyQuery: keyQuery.trim() || undefined,
    wireApi: wireApi ? wireApi as RemoteUsageQuery["wireApi"] : undefined,
    success: status === "all" ? undefined : status === "success",
    errorCategory: errorQuery.trim() || undefined,
    requestIdQuery: requestQuery.trim() || undefined,
  }), [page, range, modelQuery, connectionQuery, keyQuery, wireApi, status, errorQuery, requestQuery]);

  useEffect(() => {
    if (mode !== "remote" || !remoteUsageSupported) return;
    let active = true;
    const timer = window.setTimeout(() => {
      setRemoteLoading(true);
      setRemoteError(false);
      loadRemoteUsage(remoteQuery)
        .catch(() => active && setRemoteError(true))
        .finally(() => active && setRemoteLoading(false));
    }, 200);
    return () => { active = false; window.clearTimeout(timer); };
  }, [mode, remoteUsageSupported, remoteQuery, loadRemoteUsage]);

  useEffect(() => {
    setPage(1);
    setSelected(null);
  }, [mode]);

  const accountLabels = useMemo(() => new Map(runtime?.accounts.map((account) => [account.id, account.label]) ?? []), [runtime?.accounts]);
  const sourceLabels = useMemo(() => new Map(runtime?.sources.map((source) => [source.id, source.name]) ?? []), [runtime?.sources]);
  const rows = useMemo<UsageRow[]>(() => {
    if (mode === "zenith") return readyUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.status === "success", model: item.modelDisplay || item.model, connection: "Zenith API", key: "Zenith API", wireApi: null, ttft: item.timeToFirstByteMs ?? null, duration: item.streamDurationMs ?? item.timeToFirstByteMs ?? 0, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.status === "success" ? 200 : null, errorCategory: item.status === "success" ? null : item.status, routing: null }));
    if (mode === "remote") return remoteUsage.map((item) => ({ id: item.id, time: new Date(item.createdAtMs).toISOString(), success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.candidateLabel ?? item.candidateHint, key: item.localKeyId, wireApi: item.wireApi, ttft: item.ttftMs ?? null, duration: item.latencyMs, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory, routing: item.routing ?? null }));
    return localUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.accountId ? accountLabels.get(item.accountId) ?? item.accountId : sourceLabels.get(item.sourceId) ?? item.sourceId, key: item.localKeyId, wireApi: item.wireApi, ttft: item.ttftMs, duration: item.latencyMs, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory, routing: item.routing ?? null }));
  }, [mode, readyUsage, remoteUsage, localUsage, accountLabels, sourceLabels]);
  const cutoff = range === "all" ? 0 : Date.now() - (range === "daily" ? 1 : range === "weekly" ? 7 : 30) * 24 * 60 * 60 * 1_000;
  const filtered = mode === "remote" ? rows : rows.filter((item) =>
    (status === "all" || (status === "success" ? item.success : !item.success))
    && (!requestQuery.trim() || item.requestId?.toLocaleLowerCase().includes(requestQuery.trim().toLocaleLowerCase()))
    && (!modelQuery.trim() || item.model?.toLocaleLowerCase().includes(modelQuery.trim().toLocaleLowerCase()))
    && (!connectionQuery.trim() || item.connection.toLocaleLowerCase().includes(connectionQuery.trim().toLocaleLowerCase()))
    && (!keyQuery.trim() || item.key.toLocaleLowerCase().includes(keyQuery.trim().toLocaleLowerCase()))
    && (!wireApi || item.wireApi === wireApi)
    && (!errorQuery.trim() || item.errorCategory === errorQuery.trim())
    && new Date(item.time).getTime() >= cutoff);
  const success = rows.filter((item) => item.success).length;
  const firstResponses = rows.flatMap((item) => item.ttft == null ? [] : [item.ttft]);
  const averageFirstResponse = firstResponses.length ? Math.round(firstResponses.reduce((sum, value) => sum + value, 0) / firstResponses.length) : null;
  const averageDuration = rows.length ? Math.round(rows.reduce((sum, item) => sum + item.duration, 0) / rows.length) : null;
  const formatTime = (value: string) => new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(value));
  const resetPage = (work: () => void) => { work(); setPage(1); setSelected(null); };
  const exportRows = () => perform("usage-export", () => relayCommands.exportUsage(rows.map((row) => ({ time: row.time, success: row.success, model: row.model, connection: row.connection, latencyMs: row.duration, ttftMs: row.ttft, inputTokens: row.inputTokens, cachedInputTokens: row.cachedInputTokens, reasoningTokens: row.reasoningTokens, outputTokens: row.outputTokens, tokens: row.tokens, requestId: row.requestId, httpStatus: row.httpStatus, errorCategory: row.errorCategory }))), "feedback.exported");
  const clearLogs = async () => {
    if (!window.confirm(t("usage.clearConfirm"))) return;
    setPage(1);
    await perform("usage-clear", () => mode === "local" ? relayCommands.clearLocalUsage() : relayCommands.remoteAction({ type: "clear_usage" }), "feedback.cleared");
  };
  const canClear = mode === "local" || (mode === "remote" && remoteUsageSupported);
  const refreshUsage = () => mode === "remote" ? loadRemoteUsage(remoteQuery) : refresh();
  const clearFilters = () => {
    setStatus("all"); setRange("weekly"); setModelQuery(""); setConnectionQuery("");
    setKeyQuery(""); setWireApi(""); setErrorQuery(""); setRequestQuery("");
    setPage(1); setSelected(null);
  };

  if (mode === "remote" && !remoteUsageSupported) {
    return <section className="relay-page"><PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} /><EmptyState title={t("common.unsupported")} description={t("remote.capabilityUnavailable")} /></section>;
  }

  return <section className="relay-page">
    <PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} actions={<><ActionMenu className="usage-overflow"><ActionMenuItem danger icon={<Trash2 aria-hidden />} disabled={!canClear} title={!canClear ? t("usage.clearUnavailable") : undefined} onClick={clearLogs}>{t("usage.clearLogs")}</ActionMenuItem></ActionMenu><Button variant="secondary" icon={<Download aria-hidden />} busy={busy === "usage-export"} onClick={exportRows}>{t("common.export")}</Button><Button variant="primary" icon={<RefreshCw aria-hidden />} busy={loading || remoteLoading} onClick={refreshUsage}>{t("common.refresh")}</Button></>} />
    <Tabs value={view} onChange={(id) => { setView(id as View); setSelected(null); }} label={t("usage.views")} items={[{ id: "requests", label: t("usage.requests") }, { id: "models", label: t("common.models") }, { id: "connections", label: t("usage.poolMembers") }, { id: "errors", label: t("overview.errors") }]} />
    <div className="metric-band usage-metrics"><div><span>{t("usage.requests")}</span><strong>{mode === "remote" ? remoteUsagePage?.total ?? 0 : rows.length}</strong></div><div><span>{t("common.success")}</span><strong>{success}</strong></div><div><span>{t("usage.totalTokens")}</span><strong>{rows.reduce((sum, item) => sum + (item.tokens ?? 0), 0)}</strong></div><div><span>{t("usage.timing")}</span><strong>{averageDuration == null ? "-" : `${averageFirstResponse ?? "-"} / ${averageDuration} ms`}</strong></div></div>
    {view === "requests" ? <RequestsView rows={filtered} status={status} setStatus={(value) => resetPage(() => setStatus(value))} range={range} setRange={(value) => resetPage(() => setRange(value))} modelQuery={modelQuery} setModelQuery={(value) => resetPage(() => setModelQuery(value))} connectionQuery={connectionQuery} setConnectionQuery={(value) => resetPage(() => setConnectionQuery(value))} keyQuery={keyQuery} setKeyQuery={(value) => resetPage(() => setKeyQuery(value))} wireApi={wireApi} setWireApi={(value) => resetPage(() => setWireApi(value))} errorQuery={errorQuery} setErrorQuery={(value) => resetPage(() => setErrorQuery(value))} requestQuery={requestQuery} setRequestQuery={(value) => resetPage(() => setRequestQuery(value))} clearFilters={clearFilters} formatTime={formatTime} onSelect={setSelected} /> : null}
    {remoteError ? <p role="alert" className="form-note error-text">{t("usage.remoteLoadFailed")}</p> : null}
    {mode === "remote" && view === "requests" && remoteUsagePage && remoteUsagePage.totalPages > 1 ? <nav className="usage-pagination" aria-label={t("usage.pagination")}><Button variant="secondary" icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || remoteLoading} onClick={() => setPage((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><span>{t("usage.page", { page: remoteUsagePage.page, total: remoteUsagePage.totalPages })}</span><Button variant="secondary" icon={<ChevronRight aria-hidden />} disabled={page >= remoteUsagePage.totalPages || remoteLoading} onClick={() => setPage((value) => value + 1)}>{t("common.continue")}</Button></nav> : null}
    {view === "models" ? <AggregateView rows={rows} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={rows} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={rows.filter((item) => !item.success)} formatTime={formatTime} onSelect={setSelected} /> : null}
    {selected ? <RequestDetails row={selected} onClose={() => setSelected(null)} /> : null}
  </section>;
}

function RequestsView({ rows, status, setStatus, range, setRange, modelQuery, setModelQuery, connectionQuery, setConnectionQuery, keyQuery, setKeyQuery, wireApi, setWireApi, errorQuery, setErrorQuery, requestQuery, setRequestQuery, clearFilters, formatTime, onSelect }: { rows: UsageRow[]; status: string; setStatus: (value: string) => void; range: Range; setRange: (value: Range) => void; modelQuery: string; setModelQuery: (value: string) => void; connectionQuery: string; setConnectionQuery: (value: string) => void; keyQuery: string; setKeyQuery: (value: string) => void; wireApi: string; setWireApi: (value: string) => void; errorQuery: string; setErrorQuery: (value: string) => void; requestQuery: string; setRequestQuery: (value: string) => void; clearFilters: () => void; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  const [showMoreFilters, setShowMoreFilters] = useState(false);
  const secondaryCount = [keyQuery, wireApi, errorQuery, requestQuery].filter(Boolean).length;
  const hasFilters = range !== "weekly" || status !== "all" || Boolean(modelQuery || connectionQuery || secondaryCount);
  return <><div className="usage-filter-panel">
    <div className="usage-filters usage-filter-primary">
      <select value={range} onChange={(event) => setRange(event.target.value as Range)} aria-label={t("usage.range")}><option value="daily">{t("usage.daily")}</option><option value="weekly">{t("usage.weekly")}</option><option value="monthly">{t("usage.monthly")}</option><option value="all">{t("common.all")}</option></select>
      <select value={status} onChange={(event) => setStatus(event.target.value)} aria-label={t("common.status")}><option value="all">{t("usage.anyStatus")}</option><option value="success">{t("common.success")}</option><option value="failed">{t("common.failed")}</option></select>
      <input value={modelQuery} onChange={(event) => setModelQuery(event.target.value)} aria-label={t("common.model")} placeholder={t("common.model")} />
      <input value={connectionQuery} onChange={(event) => setConnectionQuery(event.target.value)} aria-label={t("usage.poolMember")} placeholder={t("usage.poolMember")} />
    </div>
    <div className="usage-filter-controls">{hasFilters ? <IconButton label={t("usage.clearFilters")} icon={<X aria-hidden />} onClick={clearFilters} /> : null}<span className="usage-filter-toggle-wrap"><IconButton className="usage-filter-toggle" label={t("usage.moreFilters")} icon={<SlidersHorizontal aria-hidden />} aria-expanded={showMoreFilters} onClick={() => setShowMoreFilters((current) => !current)} />{secondaryCount ? <small>{secondaryCount}</small> : null}</span></div>
    {showMoreFilters ? <div className="usage-filters usage-filter-secondary">
      <input value={keyQuery} onChange={(event) => setKeyQuery(event.target.value)} aria-label={t("usage.localKey")} placeholder={t("usage.localKey")} />
      <select value={wireApi} onChange={(event) => setWireApi(event.target.value)} aria-label={t("usage.protocol")}><option value="">{t("usage.anyProtocol")}</option><option value="responses">Responses</option><option value="chat_completions">Chat Completions</option></select>
      <input value={errorQuery} onChange={(event) => setErrorQuery(event.target.value)} aria-label={t("usage.errorCategory")} placeholder={t("usage.errorCategory")} />
      <input value={requestQuery} onChange={(event) => setRequestQuery(event.target.value)} aria-label={t("usage.requestId")} placeholder={t("usage.requestId")} />
    </div> : null}
  </div>{rows.length ? <div className="relay-table-wrap"><table className="relay-table usage-request-table"><thead><tr><th>{t("usage.time")}</th><th>{t("common.status")}</th><th>{t("common.model")}</th><th>{t("usage.poolMember")}</th><th>{t("usage.timing")}</th><th>{t("usage.tokens")}</th><th>{t("usage.requestId")}</th></tr></thead><tbody>{rows.map((item) => <tr key={item.id}><td>{formatTime(item.time)}</td><td><StatusBadge status={item.success ? "ready" : "error"} label={item.success ? t("common.success") : t("common.failed")} /></td><td><code>{item.model ?? "-"}</code></td><td>{item.connection}</td><td>{formatTiming(item.ttft, item.duration)}</td><td>{item.tokens ?? "-"}</td><td><button type="button" className="request-link request-disclosure" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${item.requestId ?? "-"}`} onClick={() => onSelect(item)}><code>{item.requestId ?? "-"}</code><ChevronRight aria-hidden /></button></td></tr>)}</tbody></table></div> : <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />}</>;
}

function formatTiming(ttft: number | null, duration: number) { return `${ttft ?? "-"} / ${duration} ms`; }

function AggregateView({ rows, field, empty }: { rows: UsageRow[]; field: "model" | "connection"; empty: string }) {
  const { t } = useTranslation();
  const groups = [...rows.reduce((map, row) => { const key = row[field] || t("common.unknown"); const current = map.get(key) ?? { name: key, requests: 0, success: 0, inputTokens: 0, cachedInputTokens: 0, reasoningTokens: 0, outputTokens: 0, tokens: 0, ttft: 0, ttftCount: 0, duration: 0 }; current.requests += 1; current.success += Number(row.success); current.inputTokens += row.inputTokens ?? 0; current.cachedInputTokens += row.cachedInputTokens ?? 0; current.reasoningTokens += row.reasoningTokens ?? 0; current.outputTokens += row.outputTokens ?? 0; current.tokens += row.tokens ?? 0; if (row.ttft != null) { current.ttft += row.ttft; current.ttftCount += 1; } current.duration += row.duration; map.set(key, current); return map; }, new Map<string, { name: string; requests: number; success: number; inputTokens: number; cachedInputTokens: number; reasoningTokens: number; outputTokens: number; tokens: number; ttft: number; ttftCount: number; duration: number }>()).values()];
  if (!groups.length) return <EmptyState title={t("usage.emptyTitle")} description={empty} />;
  return <div className="relay-table-wrap"><table className="relay-table usage-aggregate-table"><thead><tr><th>{field === "model" ? t("common.model") : t("usage.poolMember")}</th><th>{t("usage.requests")}</th><th>{t("common.success")}</th><th>{t("usage.tokens")}</th><th>{t("usage.totalTokens")}</th><th>{t("usage.timing")}</th></tr></thead><tbody>{groups.map((group) => <tr key={group.name}><td><code>{group.name}</code></td><td>{group.requests}</td><td>{Math.round(group.success / group.requests * 100)}%</td><td><div className="usage-token-breakdown"><span title={t("usage.inputTokens")}><small>{t("usage.inputShort")}</small>{group.inputTokens}</span><span title={t("usage.cachedInputTokens")}><small>{t("usage.cachedShort")}</small>{group.cachedInputTokens}</span><span title={t("usage.reasoningTokens")}><small>{t("usage.reasoningShort")}</small>{group.reasoningTokens}</span><span title={t("usage.outputTokens")}><small>{t("usage.outputShort")}</small>{group.outputTokens}</span></div></td><td>{group.tokens}</td><td>{formatTiming(group.ttftCount ? Math.round(group.ttft / group.ttftCount) : null, Math.round(group.duration / group.requests))}</td></tr>)}</tbody></table></div>;
}

function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  if (!rows.length) return <EmptyState title={t("usage.noErrors")} description={t("usage.noErrorsHint")} />;
  return <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("usage.time")}</th><th>{t("common.model")}</th><th>{t("usage.poolMember")}</th><th>{t("usage.errorCategory")}</th><th>{t("usage.requestId")}</th></tr></thead><tbody>{rows.map((row) => <tr key={row.id}><td>{formatTime(row.time)}</td><td><code>{row.model ?? "-"}</code></td><td>{row.connection}</td><td>{row.errorCategory ?? t("common.unknown")}</td><td><button type="button" className="request-link request-disclosure" aria-haspopup="dialog" aria-label={`${t("usage.requestDetails")}: ${row.requestId ?? "-"}`} onClick={() => onSelect(row)}><code>{row.requestId ?? "-"}</code><ChevronRight aria-hidden /></button></td></tr>)}</tbody></table></div>;
}

function RequestDetails({ row, onClose }: { row: UsageRow; onClose: () => void }) {
  const { t } = useTranslation();
  const routing = row.routing;
  return <Dialog title={t("usage.requestDetails")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}><dl className="detail-list"><div><dt>{t("usage.requestId")}</dt><dd><code>{row.requestId ?? "-"}</code></dd></div><div><dt>{t("common.status")}</dt><dd>{row.success ? t("common.success") : t("common.failed")}</dd></div><div><dt>{t("common.model")}</dt><dd><code>{row.model ?? "-"}</code></dd></div><div><dt>{t("usage.poolMember")}</dt><dd>{row.connection}</dd></div><div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div><div><dt>{t("usage.errorCategory")}</dt><dd>{row.errorCategory ?? "-"}</dd></div><div><dt>{t("usage.firstResponse")}</dt><dd>{row.ttft == null ? "-" : `${row.ttft} ms`}</dd></div><div><dt>{t("usage.totalTime")}</dt><dd>{row.duration} ms</dd></div><div><dt>{t("usage.inputTokens")}</dt><dd>{row.inputTokens ?? "-"}</dd></div><div><dt>{t("usage.cachedInputTokens")}</dt><dd>{row.cachedInputTokens ?? "-"}</dd></div><div><dt>{t("usage.reasoningTokens")}</dt><dd>{row.reasoningTokens ?? "-"}</dd></div><div><dt>{t("usage.outputTokens")}</dt><dd>{row.outputTokens ?? "-"}</dd></div><div><dt>{t("usage.totalTokens")}</dt><dd>{row.tokens ?? "-"}</dd></div>{routing ? <><div className="detail-section-heading"><dt>{t("usage.routingDiagnostics")}</dt><dd /></div><div><dt>{t("usage.routingReason")}</dt><dd>{t(`usage.routingReasons.${routing.reason}`)}</dd></div><div><dt>{t("usage.eligibleCandidates")}</dt><dd>{routing.eligibleCandidates}</dd></div><div><dt>{t("usage.quotaAtSelection")}</dt><dd>{routing.quotaRemainingBasisPoints == null ? t("common.unknown") : `${(routing.quotaRemainingBasisPoints / 100).toFixed(2)}%`}</dd></div><div><dt>{t("usage.inFlightAtSelection")}</dt><dd>{routing.inFlightBefore}</dd></div><div><dt>{t("usage.effectiveWeight")}</dt><dd>{routing.effectiveWeight}</dd></div><div><dt>{t("usage.dispatchesBefore")}</dt><dd>{routing.dispatchesBefore}</dd></div></> : null}</dl><p className="form-note">{t("usage.redactionHint")}</p></Dialog>;
}
