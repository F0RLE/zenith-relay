import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, ChevronRight, Download, MoreHorizontal, RefreshCw, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RemoteUsageQuery } from "../../api/types";
import { Button, EmptyState, IconButton, PageHeader, StatusBadge, Tabs } from "../../components/Ui";
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
  latency: number;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
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

  const rows = useMemo<UsageRow[]>(() => {
    if (mode === "zenith") return readyUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.status === "success", model: item.modelDisplay || item.model, connection: "Zenith API", key: "Zenith API", wireApi: null, latency: item.streamDurationMs ?? item.timeToFirstByteMs ?? 0, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.status === "success" ? 200 : null, errorCategory: item.status === "success" ? null : item.status }));
    if (mode === "remote") return remoteUsage.map((item) => ({ id: item.id, time: new Date(item.createdAtMs).toISOString(), success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.candidateHint, key: item.localKeyId, wireApi: item.wireApi, latency: item.latencyMs, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory }));
    return localUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.accountId ? t("pool.types.account") : item.sourceId, key: item.localKeyId, wireApi: item.wireApi, latency: item.latencyMs, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory }));
  }, [mode, readyUsage, remoteUsage, localUsage, t]);
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
  const formatTime = (value: string) => new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(value));
  const resetPage = (work: () => void) => { work(); setPage(1); setSelected(null); };
  const exportRows = () => perform("usage-export", () => relayCommands.exportUsage(rows.map((row) => ({ time: row.time, success: row.success, model: row.model, connection: row.connection, latencyMs: row.latency, tokens: row.tokens, requestId: row.requestId, httpStatus: row.httpStatus, errorCategory: row.errorCategory }))), "feedback.exported");
  const clearLogs = async () => {
    if (!window.confirm(t("usage.clearConfirm"))) return;
    setPage(1);
    await perform("usage-clear", () => mode === "local" ? relayCommands.clearLocalUsage() : relayCommands.remoteAction({ type: "clear_usage" }), "feedback.cleared");
  };
  const canClear = mode === "local" || (mode === "remote" && remoteUsageSupported);
  const refreshUsage = () => mode === "remote" ? loadRemoteUsage(remoteQuery) : refresh();

  if (mode === "remote" && !remoteUsageSupported) {
    return <section className="relay-page"><PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} /><EmptyState title={t("common.unsupported")} description={t("remote.capabilityUnavailable")} /></section>;
  }

  return <section className="relay-page">
    <PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} actions={<><details className="usage-overflow"><summary aria-label={t("common.actions")} title={t("common.actions")}><MoreHorizontal aria-hidden /></summary><div role="menu"><button type="button" role="menuitem" disabled={!canClear} title={!canClear ? t("usage.clearUnavailable") : undefined} onClick={clearLogs}>{t("usage.clearLogs")}</button></div></details><Button variant="secondary" icon={<Download aria-hidden />} busy={busy === "usage-export"} onClick={exportRows}>{t("common.export")}</Button><Button variant="primary" icon={<RefreshCw aria-hidden />} busy={loading || remoteLoading} onClick={refreshUsage}>{t("common.refresh")}</Button></>} />
    <Tabs value={view} onChange={(id) => { setView(id as View); setSelected(null); }} label={t("usage.views")} items={[{ id: "requests", label: t("usage.requests") }, { id: "models", label: t("common.models") }, { id: "connections", label: t("nav.connections") }, { id: "errors", label: t("overview.errors") }]} />
    <div className="metric-band usage-metrics"><div><span>{t("usage.requests")}</span><strong>{mode === "remote" ? remoteUsagePage?.total ?? 0 : rows.length}</strong></div><div><span>{t("common.success")}</span><strong>{success}</strong></div><div><span>{t("usage.tokens")}</span><strong>{rows.reduce((sum, item) => sum + (item.tokens ?? 0), 0)}</strong></div><div><span>{t("usage.latency")}</span><strong>{rows.length ? `${Math.round(rows.reduce((sum, item) => sum + item.latency, 0) / rows.length)} ms` : "-"}</strong></div></div>
    {view === "requests" ? <RequestsView rows={filtered} status={status} setStatus={(value) => resetPage(() => setStatus(value))} range={range} setRange={(value) => resetPage(() => setRange(value))} modelQuery={modelQuery} setModelQuery={(value) => resetPage(() => setModelQuery(value))} connectionQuery={connectionQuery} setConnectionQuery={(value) => resetPage(() => setConnectionQuery(value))} keyQuery={keyQuery} setKeyQuery={(value) => resetPage(() => setKeyQuery(value))} wireApi={wireApi} setWireApi={(value) => resetPage(() => setWireApi(value))} errorQuery={errorQuery} setErrorQuery={(value) => resetPage(() => setErrorQuery(value))} requestQuery={requestQuery} setRequestQuery={(value) => resetPage(() => setRequestQuery(value))} formatTime={formatTime} onSelect={setSelected} /> : null}
    {remoteError ? <p role="alert" className="form-note error-text">{t("usage.remoteLoadFailed")}</p> : null}
    {mode === "remote" && view === "requests" && remoteUsagePage && remoteUsagePage.totalPages > 1 ? <nav className="usage-pagination" aria-label={t("usage.pagination")}><Button variant="secondary" icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || remoteLoading} onClick={() => setPage((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><span>{t("usage.page", { page: remoteUsagePage.page, total: remoteUsagePage.totalPages })}</span><Button variant="secondary" icon={<ChevronRight aria-hidden />} disabled={page >= remoteUsagePage.totalPages || remoteLoading} onClick={() => setPage((value) => value + 1)}>{t("common.continue")}</Button></nav> : null}
    {view === "models" ? <AggregateView rows={rows} field="model" empty={t("usage.empty")} /> : null}
    {view === "connections" ? <AggregateView rows={rows} field="connection" empty={t("usage.empty")} /> : null}
    {view === "errors" ? <ErrorsView rows={rows.filter((item) => !item.success)} formatTime={formatTime} onSelect={setSelected} /> : null}
    {selected ? <RequestDetails row={selected} onClose={() => setSelected(null)} /> : null}
  </section>;
}

function RequestsView({ rows, status, setStatus, range, setRange, modelQuery, setModelQuery, connectionQuery, setConnectionQuery, keyQuery, setKeyQuery, wireApi, setWireApi, errorQuery, setErrorQuery, requestQuery, setRequestQuery, formatTime, onSelect }: { rows: UsageRow[]; status: string; setStatus: (value: string) => void; range: Range; setRange: (value: Range) => void; modelQuery: string; setModelQuery: (value: string) => void; connectionQuery: string; setConnectionQuery: (value: string) => void; keyQuery: string; setKeyQuery: (value: string) => void; wireApi: string; setWireApi: (value: string) => void; errorQuery: string; setErrorQuery: (value: string) => void; requestQuery: string; setRequestQuery: (value: string) => void; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  return <><div className="table-toolbar usage-filters"><select value={range} onChange={(event) => setRange(event.target.value as Range)} aria-label={t("usage.range")}><option value="daily">{t("usage.daily")}</option><option value="weekly">{t("usage.weekly")}</option><option value="monthly">{t("usage.monthly")}</option><option value="all">{t("common.all")}</option></select><select value={status} onChange={(event) => setStatus(event.target.value)} aria-label={t("common.status")}><option value="all">{t("common.all")}</option><option value="success">{t("common.success")}</option><option value="failed">{t("common.failed")}</option></select><input value={modelQuery} onChange={(event) => setModelQuery(event.target.value)} aria-label={t("common.model")} placeholder={t("common.model")} /><input value={connectionQuery} onChange={(event) => setConnectionQuery(event.target.value)} aria-label={t("usage.connection")} placeholder={t("usage.connection")} /><input value={keyQuery} onChange={(event) => setKeyQuery(event.target.value)} aria-label={t("usage.localKey")} placeholder={t("usage.localKey")} /><select value={wireApi} onChange={(event) => setWireApi(event.target.value)} aria-label={t("usage.protocol")}><option value="">{t("common.all")}</option><option value="responses">Responses</option><option value="chat_completions">Chat Completions</option></select><input value={errorQuery} onChange={(event) => setErrorQuery(event.target.value)} aria-label={t("usage.errorCategory")} placeholder={t("usage.errorCategory")} /><input value={requestQuery} onChange={(event) => setRequestQuery(event.target.value)} aria-label={t("usage.requestId")} placeholder={t("usage.requestId")} /></div>{rows.length ? <div className="relay-table-wrap"><table className="relay-table usage-request-table"><thead><tr><th>{t("usage.time")}</th><th>{t("common.status")}</th><th>{t("common.model")}</th><th>{t("nav.connections")}</th><th>{t("usage.latency")}</th><th>{t("usage.tokens")}</th><th>{t("usage.requestId")}</th></tr></thead><tbody>{rows.map((item) => <tr key={item.id}><td>{formatTime(item.time)}</td><td><StatusBadge status={item.success ? "ready" : "error"} label={item.success ? t("common.success") : t("common.failed")} /></td><td><code>{item.model ?? "-"}</code></td><td>{item.connection}</td><td>{item.latency} ms</td><td>{item.tokens ?? "-"}</td><td><button type="button" className="request-link" onClick={() => onSelect(item)}><code>{item.requestId ?? "-"}</code></button></td></tr>)}</tbody></table></div> : <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />}</>;
}

function AggregateView({ rows, field, empty }: { rows: UsageRow[]; field: "model" | "connection"; empty: string }) {
  const { t } = useTranslation();
  const groups = [...rows.reduce((map, row) => { const key = row[field] || t("common.unknown"); const current = map.get(key) ?? { name: key, requests: 0, success: 0, tokens: 0, latency: 0 }; current.requests += 1; current.success += Number(row.success); current.tokens += row.tokens ?? 0; current.latency += row.latency; map.set(key, current); return map; }, new Map<string, { name: string; requests: number; success: number; tokens: number; latency: number }>()).values()];
  if (!groups.length) return <EmptyState title={t("usage.emptyTitle")} description={empty} />;
  return <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{field === "model" ? t("common.model") : t("usage.connection")}</th><th>{t("usage.requests")}</th><th>{t("common.success")}</th><th>{t("usage.tokens")}</th><th>{t("usage.latency")}</th></tr></thead><tbody>{groups.map((group) => <tr key={group.name}><td><code>{group.name}</code></td><td>{group.requests}</td><td>{Math.round(group.success / group.requests * 100)}%</td><td>{group.tokens}</td><td>{Math.round(group.latency / group.requests)} ms</td></tr>)}</tbody></table></div>;
}

function ErrorsView({ rows, formatTime, onSelect }: { rows: UsageRow[]; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  if (!rows.length) return <EmptyState title={t("usage.noErrors")} description={t("usage.noErrorsHint")} />;
  return <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("usage.time")}</th><th>{t("common.model")}</th><th>{t("usage.connection")}</th><th>{t("usage.errorCategory")}</th><th>{t("usage.requestId")}</th></tr></thead><tbody>{rows.map((row) => <tr key={row.id}><td>{formatTime(row.time)}</td><td><code>{row.model ?? "-"}</code></td><td>{row.connection}</td><td>{row.errorCategory ?? t("common.unknown")}</td><td><button type="button" className="request-link" onClick={() => onSelect(row)}><code>{row.requestId ?? "-"}</code></button></td></tr>)}</tbody></table></div>;
}

function RequestDetails({ row, onClose }: { row: UsageRow; onClose: () => void }) {
  const { t } = useTranslation();
  return <aside className="request-details" role="dialog" aria-modal="false" aria-labelledby="request-details-title"><header><h2 id="request-details-title">{t("usage.requestDetails")}</h2><IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={onClose} /></header><dl className="detail-list"><div><dt>{t("usage.requestId")}</dt><dd><code>{row.requestId ?? "-"}</code></dd></div><div><dt>{t("common.status")}</dt><dd>{row.success ? t("common.success") : t("common.failed")}</dd></div><div><dt>{t("common.model")}</dt><dd><code>{row.model ?? "-"}</code></dd></div><div><dt>{t("usage.connection")}</dt><dd>{row.connection}</dd></div><div><dt>{t("usage.httpStatus")}</dt><dd>{row.httpStatus ?? "-"}</dd></div><div><dt>{t("usage.errorCategory")}</dt><dd>{row.errorCategory ?? "-"}</dd></div><div><dt>{t("usage.latency")}</dt><dd>{row.latency} ms</dd></div><div><dt>{t("usage.tokens")}</dt><dd>{row.tokens ?? "-"}</dd></div></dl><p className="form-note">{t("usage.redactionHint")}</p></aside>;
}
