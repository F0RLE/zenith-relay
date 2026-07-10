import { useMemo, useState } from "react";
import { Download, RefreshCw, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, EmptyState, IconButton, PageHeader, StatusBadge, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type View = "requests" | "models" | "connections" | "errors";
type UsageRow = {
  id: string | number;
  time: string;
  success: boolean;
  model: string | null;
  connection: string;
  latency: number;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
};

export function UsagePage() {
  const { t, i18n } = useTranslation();
  const { mode, localUsage, remoteUsage, readyUsage, refresh, loading } = useRelayState();
  const [view, setView] = useState<View>("requests");
  const [status, setStatus] = useState("all");
  const [selected, setSelected] = useState<UsageRow | null>(null);
  const rows = useMemo<UsageRow[]>(() => {
    if (mode === "zenith") return readyUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.status === "success", model: item.modelDisplay || item.model, connection: "Zenith API", latency: item.streamDurationMs ?? item.timeToFirstByteMs ?? 0, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.status === "success" ? 200 : null, errorCategory: item.status === "success" ? null : item.status }));
    if (mode === "remote") return remoteUsage.map((item) => ({ id: item.id, time: new Date(item.createdAtMs).toISOString(), success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.candidateHint, latency: item.latencyMs, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory }));
    return localUsage.map((item) => ({ id: item.id, time: item.createdAt, success: item.success, model: item.resolvedModel ?? item.requestedModel, connection: item.accountId ? t("pool.types.account") : item.sourceId, latency: item.latencyMs, tokens: item.totalTokens, requestId: item.requestId, httpStatus: item.httpStatus, errorCategory: item.errorCategory }));
  }, [mode, readyUsage, remoteUsage, localUsage, t]);
  const filtered = status === "all" ? rows : rows.filter((item) => status === "success" ? item.success : !item.success);
  const success = rows.filter((item) => item.success).length;
  const formatTime = (value: string) => new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(value));

  return (
    <section className="relay-page">
      <PageHeader title={t("nav.usage")} subtitle={t("usage.subtitle")} actions={<><Button variant="secondary" icon={<Download aria-hidden />} disabled>{t("common.export")}</Button><Button variant="primary" icon={<RefreshCw aria-hidden />} busy={loading} onClick={refresh}>{t("common.refresh")}</Button></>} />
      <Tabs value={view} onChange={(id) => { setView(id as View); setSelected(null); }} label={t("usage.views")} items={[{ id: "requests", label: t("usage.requests") }, { id: "models", label: t("common.models") }, { id: "connections", label: t("nav.connections") }, { id: "errors", label: t("overview.errors") }]} />
      <div className="metric-band usage-metrics"><div><span>{t("usage.requests")}</span><strong>{rows.length}</strong></div><div><span>{t("common.success")}</span><strong>{success}</strong></div><div><span>{t("usage.tokens")}</span><strong>{rows.reduce((sum, item) => sum + (item.tokens ?? 0), 0)}</strong></div><div><span>{t("usage.latency")}</span><strong>{rows.length ? `${Math.round(rows.reduce((sum, item) => sum + item.latency, 0) / rows.length)} ms` : "-"}</strong></div></div>
      {view === "requests" ? <RequestsView rows={filtered} status={status} setStatus={setStatus} formatTime={formatTime} onSelect={setSelected} /> : null}
      {view === "models" ? <AggregateView rows={rows} field="model" empty={t("usage.empty")} /> : null}
      {view === "connections" ? <AggregateView rows={rows} field="connection" empty={t("usage.empty")} /> : null}
      {view === "errors" ? <ErrorsView rows={rows.filter((item) => !item.success)} formatTime={formatTime} onSelect={setSelected} /> : null}
      {selected ? <RequestDetails row={selected} onClose={() => setSelected(null)} /> : null}
    </section>
  );
}

function RequestsView({ rows, status, setStatus, formatTime, onSelect }: { rows: UsageRow[]; status: string; setStatus: (value: string) => void; formatTime: (value: string) => string; onSelect: (row: UsageRow) => void }) {
  const { t } = useTranslation();
  return <><div className="table-toolbar"><select value={status} onChange={(event) => setStatus(event.target.value)} aria-label={t("common.status")}><option value="all">{t("common.all")}</option><option value="success">{t("common.success")}</option><option value="failed">{t("common.failed")}</option></select><input placeholder={t("usage.requestId")} /></div>{rows.length ? <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("usage.time")}</th><th>{t("common.status")}</th><th>{t("common.model")}</th><th>{t("nav.connections")}</th><th>{t("usage.latency")}</th><th>{t("usage.tokens")}</th><th>{t("usage.requestId")}</th></tr></thead><tbody>{rows.map((item) => <tr key={item.id}><td>{formatTime(item.time)}</td><td><StatusBadge status={item.success ? "ready" : "error"} label={item.success ? t("common.success") : t("common.failed")} /></td><td><code>{item.model ?? "-"}</code></td><td>{item.connection}</td><td>{item.latency} ms</td><td>{item.tokens ?? "-"}</td><td><button type="button" className="request-link" onClick={() => onSelect(item)}><code>{item.requestId ?? "-"}</code></button></td></tr>)}</tbody></table></div> : <EmptyState title={t("usage.emptyTitle")} description={t("usage.empty")} />}</>;
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
