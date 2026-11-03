import { useMemo, useRef, useState } from "react";
import { CircleCheck, CircleX, Clock3, RefreshCw, RotateCcw, SlidersHorizontal } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { Pagination } from "../../components/Pagination";
import { inUsagePeriod, usagePeriods } from "./period";
import type { UsagePeriod } from "./period";
import { accountingLabels, executionLabels, generationSpeed, tokenTotal, usageTotals } from "./model";
import type { RequestRecord } from "./model";
import { useLocale } from "../../i18n/LocaleProvider";
import { useFormatters } from "../../i18n/format";
import type { MessageKey } from "../../i18n/types";
import "./usage-filters.css";

type View = "requests" | "models" | "keys" | "errors";
const viewNames: Record<View, MessageKey> = { requests: "common.requests", models: "nav.models", keys: "nav.keys", errors: "common.errors" };
const emptyFilters = { range: "today" as UsagePeriod, status: "all", model: "all", key: "all", protocol: "all", transport: "all", accounting: "all", query: "" };

function Metric({ label, value, note }: { label: MessageKey; value: string; note: string }) {
  const { t } = useLocale();
  return <article className="usage-metric" data-relay-tooltip={note} tabIndex={0}><div className="usage-metric-copy"><span>{t(label)}</span><strong>{value}</strong></div></article>;
}

function Status({ record }: { record: RequestRecord }) {
  const { t } = useLocale();
  const Icon = record.execution === "completed" ? CircleCheck : record.execution === "failed" ? CircleX : Clock3;
  return <span className="table-status" data-status={record.execution} role="img" aria-label={t(executionLabels[record.execution])}><Icon aria-hidden="true" /></span>;
}

export function UsagePage({ records, asOf, onSelectRequest }: { records: readonly RequestRecord[]; asOf: string; onSelectRequest: (record: RequestRecord) => void }) {
  const { t, localize } = useLocale();
  const { formatDuration, formatInteger, formatMoney, formatSpeed, formatTime, formatCompact: compact, formatPercent } = useFormatters();
  const [view, setView] = useState<View>("requests");
  const [filters, setFilters] = useState(emptyFilters);
  const [more, setMore] = useState(false);
  const [page, setPage] = useState(1);
  const [size, setSize] = useState(8);
  const filterTrigger = useRef<HTMLButtonElement>(null);
  const filterBar = useRef<HTMLDivElement>(null);
  const hasFilters = (Object.keys(emptyFilters) as (keyof typeof emptyFilters)[]).some(field => filters[field] !== emptyFilters[field]);
  const hasExtraFilters = (["protocol", "transport", "accounting", "query"] as const).some(field => filters[field] !== emptyFilters[field]);
  const models = useMemo(() => [...new Map(records.map(row => [row.model.id, row.model])).values()], [records]);
  const keys = useMemo(() => [...new Map(records.map(row => [row.key.id, row.key])).values()], [records]);
  function change(field: keyof typeof emptyFilters, value: string) { setFilters(current => ({ ...current, [field]: value })); setPage(1); }
  function resetFilters() {
    setFilters(emptyFilters); setMore(false); setPage(1);
    const target = filterTrigger.current?.getClientRects().length ? filterTrigger.current : filterBar.current?.querySelector<HTMLButtonElement>('[role="combobox"]');
    target?.focus({ preventScroll: true });
  }
  const rows = useMemo(() => {
    return records.filter(row => inUsagePeriod(row.startedAt, asOf, filters.range)
      && (view === "errors" ? row.execution === "failed" : filters.status === "all" || row.execution === filters.status)
      && (filters.model === "all" || row.model.id === filters.model) && (filters.key === "all" || row.key.id === filters.key)
      && (filters.protocol === "all" || row.protocol === filters.protocol) && (filters.transport === "all" || row.transport === filters.transport)
      && (filters.accounting === "all" || row.accounting === filters.accounting)
      && `${row.id} ${row.client.label} ${row.client.userAgent}`.toLowerCase().includes(filters.query.trim().toLowerCase()))
      .sort((a, b) => Date.parse(b.startedAt) - Date.parse(a.startedAt));
  }, [records, asOf, filters, view]);
  const summary = useMemo(() => usageTotals(rows), [rows]);
  const grouped = view === "models" || view === "keys";
  const groups = useMemo(() => {
    if (!grouped) return [];
    const buckets = new Map<string, { id: string; name: string; rows: RequestRecord[] }>();
    for (const row of rows) {
      const item = view === "models" ? row.model : row.key;
      let group = buckets.get(item.id);
      if (!group) { group = { id: item.id, name: item.name, rows: [] }; buckets.set(item.id, group); }
      group.rows.push(row);
    }
    return [...buckets.values()].map(item => ({ id: item.id, name: item.name, totals: usageTotals(item.rows) }));
  }, [grouped, rows, view]);
  const maxPage = Math.max(1, Math.ceil((grouped ? groups.length : rows.length) / size));
  const currentPage = Math.min(page, maxPage);
  const displayed = rows.slice((currentPage - 1) * size, currentPage * size);

  return <section className="usage-page">
    <header className="relay-page-header relay-workspace-header"><h1>{t("nav.usage")}</h1><div className="relay-page-navigation"><div className="relay-tabs" role="group" aria-label={t("usage.usageView")}>{(Object.keys(viewNames) as View[]).map(id => <button type="button" key={id} className={view === id ? "active" : ""} aria-pressed={view === id} onClick={() => { setView(id); setPage(1); }}>{t(viewNames[id])}</button>)}</div></div>
    </header>
    <div className="usage-filters" ref={filterBar}>
      <label>{t("usage.periodUtc")}<OptionMenu label={t("usage.periodUtc")} value={filters.range} onChange={value => change("range", value)} options={usagePeriods.map(item => ({ ...item, label: t(item.label), ...("shortLabel" in item ? { shortLabel: t(item.shortLabel) } : {}) }))} /></label>
      <label>{t("usage.execution")}<OptionMenu label={t("usage.execution")} disabled={view === "errors"} value={view === "errors" ? "failed" : filters.status} onChange={value => change("status", value)} options={[{ value: "all", label: t("billing.allStatuses") }, ...Object.entries(executionLabels).map(([value, label]) => ({ value, label: t(label) }))]} /></label>
      <label>{t("common.model")}<OptionMenu label={t("common.model")} value={filters.model} onChange={value => change("model", value)} options={[{ value: "all", label: t("usage.allModels") }, ...models.map(model => ({ value: model.id, label: model.name }))]} /></label>
      <div className="usage-key-controls"><label>{t("common.apiKey")}<OptionMenu label={t("common.apiKey")} value={filters.key} onChange={value => change("key", value)} options={[{ value: "all", label: t("usage.allKeys") }, ...keys.map(key => ({ value: key.id, label: key.name }))]} /></label>
        <button ref={filterTrigger} type="button" className="icon-button usage-filter-trigger" aria-label={t("usage.additionalFilters")} aria-expanded={more} aria-controls="usage-extra-filters" data-active={hasExtraFilters || undefined} onClick={() => setMore(!more)}><SlidersHorizontal aria-hidden="true" /></button>
        <button type="button" className="icon-button usage-refresh" aria-label={t("usage.refreshUsage")}><RefreshCw aria-hidden="true" /></button></div>
    <div className="usage-extra-filters" id="usage-extra-filters" data-expanded={more}>
      <label>{t("common.protocol")}<OptionMenu label={t("common.protocol")} value={filters.protocol} onChange={value => change("protocol", value)} options={[{ value: "all", label: t("usage.allProtocols"), shortLabel: t("common.protocol") }, ...[...new Set(records.map(row => row.protocol))].map(value => ({ value, label: value }))]} /></label>
      <label>{t("usage.transport")}<OptionMenu label={t("usage.transport")} value={filters.transport} onChange={value => change("transport", value)} options={[{ value: "all", label: t("usage.any"), shortLabel: t("usage.transport") }, ...["HTTP", "WebSocket"].map(value => ({ value, label: value }))]} /></label>
      <label>{t("usage.accounting")}<OptionMenu label={t("usage.accounting")} value={filters.accounting} onChange={value => change("accounting", value)} options={[{ value: "all", label: t("connections.allStates"), shortLabel: t("usage.accounting") }, ...Object.entries(accountingLabels).map(([value, label]) => ({ value, label: t(label) }))]} /></label>
      <label>{t("usage.requestIdOrClient")}<input type="search" value={filters.query} onChange={event => change("query", event.target.value)} placeholder={t("common.search")} /></label>
      {hasFilters && <div className="usage-filter-footer"><button type="button" className="text-button usage-filter-reset" aria-label={t("usage.resetFilters")} data-relay-tooltip={t("usage.resetFilters")} onClick={resetFilters}><RotateCcw aria-hidden="true" /><span>{t("usage.resetFilters")}</span></button></div>}
    </div>
    </div>
    <section className="usage-overview" aria-label={t("usage.usageSummary")}><div className="usage-metrics">
      <Metric label="common.requests" value={formatInteger(summary.count)} note={t("usage.logicalRequests")} />
      {!grouped && <Metric label="usage.successful" value={summary.finished ? formatPercent(summary.success / summary.finished) : "—"} note={t("usage.successCount", { success: formatInteger(summary.success), finished: formatInteger(summary.finished) })} />}
      <Metric label="usage.totalTokens" value={summary.known ? compact(summary.tokens) : "—"} note={t("usage.tokenCoverage", { known: formatInteger(summary.known), total: formatInteger(summary.count) })} />
      <Metric label="common.charged" value={`${formatMoney(summary.charge)} USD`} note={summary.charge === null ? t("usage.someFinalChargesAreUnavailable") : summary.pending ? t("usage.awaitingFinalAccountingCount", { count: formatInteger(summary.pending) }) : t("usage.postedChargesOnly")} />
    </div><div className="usage-overview-details"><dl className="usage-token-totals">{([["input", "usage.inputTokens"], ["output", "usage.outputTokens"], ["cacheRead", "usage.cacheRead"], ["cacheWrite", "usage.cacheWrite"]] as const).map(([field, label]) => <div key={field}><dt>{t(label)}</dt><dd>{rows.some(row => row.tokens[field] !== null) ? compact(rows.reduce((sum, row) => sum + (row.tokens[field] ?? 0), 0)) : "—"}</dd></div>)}</dl><div className="usage-secondary-metrics"><Metric label="usage.generationSpeed" value={formatSpeed(summary.generation)} note={t("usage.measuredResponses")} /><Metric label="usage.includingWaitTime" value={formatSpeed(summary.e2e)} note={t("usage.includesTimeBeforeTheResponse")} /></div></div></section>
    <div className="table-heading sr-only"><h2>{t(grouped ? view === "models" ? "usage.usageByModel" : "usage.usageByApiKey" : viewNames[view])}</h2><small>{filters.key === "all" ? t("usage.allKeys") : keys.find(key => key.id === filters.key)?.name}</small></div>
    <div className="table-shell"><div className="table-scroll" role="region" aria-label={t("usage.usageTable")}>
      {rows.length === 0 ? <div className="empty-state"><strong>{t("usage.noRequestsFound")}</strong><p>{t("usage.changeTheCriteriaOrResetFilters")}</p></div> : grouped ? <table className="aggregate-table"><thead><tr><th>{t(view === "models" ? "common.model" : "common.apiKey")}</th><th>{t("common.requests")}</th><th>{t("common.tokens")}</th><th>{t("usage.chargedUsd")}</th></tr></thead><tbody>{groups.slice((currentPage - 1) * size, currentPage * size).map(group => <tr key={group.id}>
        <td><button type="button" className="text-button" onClick={() => { change(view === "models" ? "model" : "key", group.id); setView("requests"); }}>{group.name}</button></td>
        <td data-label={t("common.requests")}>{formatInteger(group.totals.count)}</td><td data-label={t("common.tokens")}>{group.totals.known ? formatInteger(group.totals.tokens) : "—"}</td>
        <td><span className="group-charge">{formatMoney(group.totals.charge)}</span>{group.totals.pending > 0 && <small>{t("usage.unsettledCount", { count: formatInteger(group.totals.pending) })}</small>}</td>
      </tr>)}</tbody></table>
        : <table className="request-table"><colgroup>{["time", "status", "model", "protocol", "tier", "key", "timing", "speed", "tokens", "cost", "request"].map(id => <col key={id} data-column={id} />)}</colgroup><thead><tr>{([["time", "common.time"], ["status", "common.status"], ["model", "common.model"], ["protocol", "common.protocol"], ["tier", "usage.requestSpeed"], ["key", "common.apiKey"], ["timing", "usage.firstTotal"], ["speed", "usage.speed"], ["tokens", "common.tokens"], ["cost", "usage.chargedUsd"], ["request", "usage.requestIdentifier"]] as const).map(([id, title]) => <th key={id} data-column={id}>{t(title)}</th>)}</tr></thead>
          <tbody>{displayed.map(record => <tr key={record.id}>
            <td data-column="time" data-label={t("usage.timeUtc")}><time dateTime={record.startedAt}>{formatTime(record.startedAt)}</time></td><td data-column="status"><Status record={record} /></td>
            <td data-column="model" data-label={t("common.model")}><strong className="model-name">{record.model.name}</strong><small>{localize(record.model.reasoningEffort)}</small></td>
            <td data-column="protocol"><code>{record.protocol}</code></td><td data-column="tier">{localize(record.tier)}</td><td data-column="key" data-label={t("common.apiKey")}>{record.key.name}<small>{record.key.mask}</small></td>
            <td data-column="timing" data-label={t("usage.firstTotal")}>{record.timing.totalMs === null ? t("usage.inProgress") : `${formatDuration(record.timing.firstTokenMs)} / ${formatDuration(record.timing.totalMs)}`}</td>
            <td data-column="speed">{formatSpeed(generationSpeed(record))}</td><td data-column="tokens" data-label={t("common.tokens")}>{tokenTotal(record.tokens) === null ? <span aria-label={t("usage.tokenCountNotReceivedYet")}>—</span> : compact(tokenTotal(record.tokens)!)}</td>
            <td data-column="cost" data-label={t("usage.chargedUsd")}>{record.billing.postedMicroUsd === null ? <span className="pending-text">{t(accountingLabels[record.accounting])}</span> : formatMoney(record.billing.postedMicroUsd)}</td>
            <td data-column="request" data-label={t("usage.requestIdentifier")}><button type="button" className="request-link" aria-haspopup="dialog" aria-label={t("usage.requestDetailsId", { id: record.id })} onClick={() => onSelectRequest(record)}><span className="request-link-id">{record.id.replace("request_", "…")}</span><span className="request-link-label">{t("usage.moreDetails")}</span></button></td>
          </tr>)}</tbody></table>}
    </div></div>
    <Pagination label={t("usage.usagePages")} count={grouped ? groups.length : rows.length} page={currentPage} size={size} sizes={[8, 16, 32]} suffix={grouped ? t("usage.requestCountSuffix", { count: formatInteger(rows.length) }) : ""} onPage={setPage} onSize={value => { setSize(value); setPage(1); }} />
  </section>;
}
