import { useMemo, useState } from "react";
import { Activity, CircleCheck, CircleX, Clock3, CreditCard, Database, Gauge, RefreshCw, SlidersHorizontal, TrendingUp, X, ChevronLeft, ChevronRight } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { OptionMenu } from "../../components/OptionMenu";
import { accountingLabels, executionLabels, formatDuration, formatInteger, formatMoney, formatSpeed, formatTime, generationSpeed, tokenTotal, usageTotals } from "./model";
import type { RequestRecord } from "./model";

type View = "requests" | "models" | "keys" | "errors";
const viewNames: Record<View, string> = { requests: "Запросы", models: "Модели", keys: "API-ключи", errors: "Ошибки" };
const emptyFilters = { range: "today", status: "all", model: "all", key: "all", protocol: "all", transport: "all", accounting: "all", query: "" };
const compact = (n: number) => n >= 1000 ? `${(n / 1000).toLocaleString("ru-RU", { maximumFractionDigits: 1 })} тыс.` : formatInteger(n);

function Metric({ icon: Icon, label, value, note }: { icon: LucideIcon; label: string; value: string; note: string }) {
  return <article className="usage-metric"><span className="metric-icon"><Icon aria-hidden="true" /></span><div><span>{label}</span><strong>{value}</strong><small>{note}</small></div></article>;
}

function Status({ record }: { record: RequestRecord }) {
  const Icon = record.execution === "completed" ? CircleCheck : record.execution === "failed" ? CircleX : Clock3;
  return <span className="table-status" data-status={record.execution} role="img" aria-label={executionLabels[record.execution]}><Icon aria-hidden="true" /></span>;
}

export function UsagePage({ records, asOf, onSelectRequest }: { records: readonly RequestRecord[]; asOf: string; onSelectRequest: (record: RequestRecord) => void }) {
  const [view, setView] = useState<View>("requests");
  const [filters, setFilters] = useState(emptyFilters);
  const [more, setMore] = useState(false);
  const [page, setPage] = useState(1);
  const [size, setSize] = useState(8);
  const [notice, setNotice] = useState("");
  const models = [...new Map(records.map(row => [row.model.id, row.model])).values()];
  const keys = [...new Map(records.map(row => [row.key.id, row.key])).values()];
  function change(field: keyof typeof emptyFilters, value: string) { setFilters(current => ({ ...current, [field]: value })); setPage(1); }
  const rows = useMemo(() => {
    const today = new Date(asOf); today.setUTCHours(0, 0, 0, 0);
    const start = filters.range === "today" ? today.getTime() : filters.range === "week" ? today.getTime() - 6 * 86400000 : filters.range === "month" ? today.getTime() - 29 * 86400000 : -Infinity;
    return records.filter(row => Date.parse(row.startedAt) >= start
      && (view === "errors" ? row.execution === "failed" : filters.status === "all" || row.execution === filters.status)
      && (filters.model === "all" || row.model.id === filters.model) && (filters.key === "all" || row.key.id === filters.key)
      && (filters.protocol === "all" || row.protocol === filters.protocol) && (filters.transport === "all" || row.transport === filters.transport)
      && (filters.accounting === "all" || row.accounting === filters.accounting)
      && `${row.id} ${row.client.label} ${row.client.userAgent}`.toLowerCase().includes(filters.query.trim().toLowerCase()));
  }, [records, asOf, filters, view]);
  const summary = usageTotals(rows);
  const grouped = view === "models" || view === "keys";
  const groups = grouped ? [...new Map(rows.map(row => { const item = view === "models" ? row.model : row.key; return [item.id, item]; })).values()].map(item => ({ ...item, totals: usageTotals(rows.filter(row => (view === "models" ? row.model.id : row.key.id) === item.id)) })) : [];
  const maxPage = Math.max(1, Math.ceil(rows.length / size));
  const currentPage = Math.min(page, maxPage);
  const displayed = rows.slice((currentPage - 1) * size, currentPage * size);

  return <>
    <header className="page-heading"><h1>Использование</h1><span>{formatTime(asOf)} UTC</span></header>
    <div className="usage-viewbar"><div className="usage-views" role="group" aria-label="Представление использования">{(Object.keys(viewNames) as View[]).map(id => <button type="button" key={id} aria-pressed={view === id} onClick={() => { setView(id); setPage(1); }}>{viewNames[id]}</button>)}</div>
      <button type="button" className="icon-button" aria-label="Обновить использование" onClick={() => setNotice("Локальный демо-снимок.")}><RefreshCw aria-hidden="true" /></button></div>
    <div className="usage-filters">
      <label>Период, UTC<OptionMenu label="Период, UTC" value={filters.range} onChange={value => change("range", value)} options={[{ value: "today", label: "Сегодня" }, { value: "week", label: "Последние 7 дней" }, { value: "month", label: "Последние 30 дней" }, { value: "all", label: "Всё время" }]} /></label>
      <label>Выполнение<OptionMenu label="Выполнение" disabled={view === "errors"} value={view === "errors" ? "failed" : filters.status} onChange={value => change("status", value)} options={[{ value: "all", label: "Все статусы" }, { value: "completed", label: "Завершён" }, { value: "failed", label: "Ошибка" }, { value: "running", label: "В работе" }]} /></label>
      <label>Модель<OptionMenu label="Модель" value={filters.model} onChange={value => change("model", value)} options={[{ value: "all", label: "Все модели" }, ...models.map(model => ({ value: model.id, label: model.name }))]} /></label>
      <label>API-ключ<OptionMenu label="API-ключ" value={filters.key} onChange={value => change("key", value)} options={[{ value: "all", label: "Все ключи" }, ...keys.map(key => ({ value: key.id, label: key.name }))]} /></label>
      <div className="filter-actions"><button type="button" className="icon-button" aria-label="Дополнительные фильтры" aria-expanded={more} aria-controls="usage-extra-filters" onClick={() => setMore(!more)}><SlidersHorizontal aria-hidden="true" /></button><button type="button" className="icon-button" aria-label="Сбросить фильтры" onClick={() => { setFilters(emptyFilters); setMore(false); setPage(1); }}><X aria-hidden="true" /></button></div>
    </div>
    {more && <div className="usage-extra-filters" id="usage-extra-filters">
      <label>Протокол<OptionMenu label="Протокол" value={filters.protocol} onChange={value => change("protocol", value)} options={[{ value: "all", label: "Все протоколы" }, ...[...new Set(records.map(row => row.protocol))].map(value => ({ value, label: value }))]} /></label>
      <label>Транспорт<OptionMenu label="Транспорт" value={filters.transport} onChange={value => change("transport", value)} options={[{ value: "all", label: "Любой" }, ...["HTTP", "WebSocket"].map(value => ({ value, label: value }))]} /></label>
      <label>Расчёт<OptionMenu label="Расчёт" value={filters.accounting} onChange={value => change("accounting", value)} options={[{ value: "all", label: "Все состояния" }, ...Object.entries(accountingLabels).map(([value, label]) => ({ value, label }))]} /></label>
      <label>ID запроса или клиент<input type="search" value={filters.query} onChange={event => change("query", event.target.value)} placeholder="Поиск" /></label>
    </div>}
    <div className="usage-summary">
      <Metric icon={Activity} label="Запросы" value={formatInteger(summary.count)} note="Логические запросы" />
      {!grouped && <Metric icon={CircleCheck} label="Успешно" value={summary.finished ? `${(summary.success / summary.finished * 100).toLocaleString("ru-RU", { maximumFractionDigits: 1 })}%` : "—"} note={`${summary.success} из ${summary.finished} с итогом`} />}
      <Metric icon={Database} label="Всего токенов" value={summary.known ? compact(summary.tokens) : "—"} note={`Токены известны для ${summary.known} из ${summary.count}`} />
      <Metric icon={CreditCard} label="Списано" value={`${formatMoney(summary.charge)} USD`} note={summary.charge === null ? "Часть итоговых сумм недоступна" : summary.pending ? `${summary.pending} ещё без итогового расчёта` : "Только проведённые списания"} />
      {!grouped && <><Metric icon={TrendingUp} label="Скорость генерации" value={formatSpeed(summary.generation)} note="По измеренным ответам" /><Metric icon={Gauge} label="С учётом ожидания" value={formatSpeed(summary.e2e)} note="Включая время до ответа" /></>}
    </div>
    <div className="table-heading"><h2>{grouped ? `Использование по ${view === "models" ? "моделям" : "API-ключам"}` : viewNames[view]}</h2><small>{filters.key === "all" ? "Все ключи" : keys.find(key => key.id === filters.key)?.name}</small></div>
    <div className="table-shell"><div className="table-scroll" role="region" aria-label="Таблица использования">
      {rows.length === 0 ? <div className="empty-state"><strong>Запросов не найдено</strong><p>Измените условия или сбросьте фильтры.</p></div> : grouped ? <table className="aggregate-table"><thead><tr><th>{view === "models" ? "Модель" : "API-ключ"}</th><th>Запросы</th><th>Токены</th><th>Списано, USD</th></tr></thead><tbody>{groups.map(group => <tr key={group.id}><td><button type="button" className="text-button" onClick={() => { change(view === "models" ? "model" : "key", group.id); setView("requests"); }}>{group.name}</button></td><td>{group.totals.count}</td><td>{group.totals.known ? formatInteger(group.totals.tokens) : "—"}</td><td>{formatMoney(group.totals.charge)}{group.totals.pending > 0 && <small>Без расчёта: {group.totals.pending}</small>}</td></tr>)}</tbody></table>
        : <table className="request-table"><colgroup>{["time", "status", "model", "protocol", "tier", "key", "timing", "speed", "tokens", "cost", "request"].map(id => <col key={id} data-column={id} />)}</colgroup><thead><tr>{[["time", "Время"], ["status", "Статус"], ["model", "Модель"], ["protocol", "Протокол"], ["tier", "Скорость запроса"], ["key", "API-ключ"], ["timing", "Первый / всего"], ["speed", "Скорость"], ["tokens", "Токены"], ["cost", "Списано, USD"], ["request", "Идентификатор запроса"]].map(([id, title]) => <th key={id} data-column={id}>{title}</th>)}</tr></thead>
          <tbody>{displayed.map(record => <tr key={record.id}>
            <td data-column="time" data-label="Время, UTC"><time dateTime={record.startedAt}>{formatTime(record.startedAt)}</time></td><td data-column="status"><Status record={record} /></td>
            <td data-column="model" data-label="Модель"><strong className="model-name">{record.model.name}</strong><small>{record.model.reasoningEffort}</small></td>
            <td data-column="protocol"><code>{record.protocol}</code></td><td data-column="tier">{record.tier}</td><td data-column="key" data-label="API-ключ">{record.key.name}<small>{record.key.mask}</small></td>
            <td data-column="timing" data-label="Первый / всего">{record.timing.totalMs === null ? "Выполняется" : `${formatDuration(record.timing.firstTokenMs)} / ${formatDuration(record.timing.totalMs)}`}</td>
            <td data-column="speed">{formatSpeed(generationSpeed(record))}</td><td data-column="tokens" data-label="Токены">{tokenTotal(record.tokens) === null ? <span aria-label="Счётчик токенов ещё не получен">—</span> : compact(tokenTotal(record.tokens)!)}</td>
            <td data-column="cost" data-label="Списано, USD">{record.billing.postedMicroUsd === null ? <span className="pending-text">{accountingLabels[record.accounting]}</span> : formatMoney(record.billing.postedMicroUsd)}</td>
            <td data-column="request" data-label="Идентификатор запроса"><button type="button" className="request-link" aria-haspopup="dialog" aria-label={`Детали запроса ${record.id}`} onClick={() => onSelectRequest(record)}>{record.id.replace("request_", "…")}</button></td>
          </tr>)}</tbody></table>}
    </div></div>
    <div className="pagination"><span role="status">{grouped ? `${groups.length} групп · ${rows.length} запросов` : rows.length ? `${(currentPage - 1) * size + 1}–${Math.min(currentPage * size, rows.length)} из ${rows.length}` : "0 запросов"}</span>
      {!grouped && rows.length > 0 && <div><label>Строк<OptionMenu label="Строк" value={String(size)} onChange={value => { setSize(Number(value)); setPage(1); }} options={[8, 16, 32].map(value => ({ value: String(value), label: String(value) }))} /></label><button type="button" className="icon-button" aria-label="Предыдущая страница" disabled={currentPage === 1} onClick={() => setPage(currentPage - 1)}><ChevronLeft aria-hidden="true" /></button><span>{currentPage} / {maxPage}</span><button type="button" className="icon-button" aria-label="Следующая страница" disabled={currentPage === maxPage} onClick={() => setPage(currentPage + 1)}><ChevronRight aria-hidden="true" /></button></div>}
    </div>
    {notice && <p role="status" className="form-note">{notice}</p>}
  </>;
}
