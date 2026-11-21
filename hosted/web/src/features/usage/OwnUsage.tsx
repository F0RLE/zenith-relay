import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { executions, isExecution, readUsageOverview, readUsagePage, reportKeyPrefix, usageLink,
  type Hours, type UsageSelection } from "../../api/usageReport";
import type { CustomerDiagnostic } from "../../api/requestDiagnostics";
import { CopyButton } from "../../components/CopyButton";
import { ChevronDown, RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { localInterval, reportView, ReportPeriods, ReportStatus, useReportRead, useReportSource } from "./ReportTools";
import { ReportExport } from "./ReportExport";
import { formatMoney } from "./model";

const outcomes = { reserved: "status.held", dispatched: "status.running", completed: "status.completed",
  incomplete: "receipt.incomplete", failed: "status.error", uncertain: "receipt.uncertain" } as const;
const accounting = { held: "status.held", pending: "report.awaitingAccounting", settled: "report.settled", released: "report.released" } as const;
const basis = { provider_final: "receipt.final", provider_partial: "receipt.partial", not_sent: "receipt.notSent", policy_waiver: "receipt.waived" } as const;
type Receipt = Awaited<ReturnType<typeof readUsagePage>>["items"][number];
type Options = Pick<Awaited<ReturnType<typeof readUsageOverview>>, "models" | "keys">;

export function OwnUsage({ csrf, onOverview }: { csrf: string; onOverview: (href: string) => void }) {
  const { t } = useLocale();
  const [view] = useState(() => reportView(false));
  const [selection, select] = useState<UsageSelection>(() => ({
    model: view?.model, bucket: view?.bucket, keyId: view?.keyId, execution: view?.execution,
  }));
  const [retryOptions, setRetryOptions] = useState(0);
  const source = useReportSource(view, csrf);
  const report = source.report;
  const id = view?.id;
  const optionsRead = useCallback((signal: AbortSignal) => readUsageOverview({ report: report! }, signal), [report, retryOptions]);
  const { data: options, error: optionsError } = useReportRead(report && !id ? optionsRead : null);
  const hours = options?.hours ?? source.hours;
  const selectionUrl = usageLink({ report, ...selection }, hours);
  useEffect(() => {
    if (!report || !options || id) return;
    history.replaceState(null, "", selectionUrl);
    onOverview(usageLink({ report, model: selection.model }, options.hours).replace("/app/usage", "/app/overview"));
  }, [report, options, id, selectionUrl, selection.model, onOverview]);

  function refresh(width: Hours = hours) {
    const next = { ...selection, bucket: undefined };
    select(next);
    history.replaceState(null, "", usageLink(next, width));
    onOverview(usageLink({ model: next.model }, width).replace("/app/usage", "/app/overview"));
    source.refresh(width);
  }
  if (!view) return <ReportStatus error="invalid_filter" retry={() => location.assign("/app/usage")} />;
  const problem = source.error ?? optionsError;
  const retry = () => {
    if (report && optionsError !== "report_not_found") setRetryOptions(n => n + 1);
    else refresh();
  };
  return <section className="own-history" aria-label={t("nav.usage")}>
    {!id && <div className="report-usage-controls">
      <ReportPeriods hours={hours} onChange={refresh} disabled={!report && !problem} />
      <button type="button" className="icon-button report-refresh" aria-label={t("usage.refreshUsage")} title={t("usage.refreshUsage")}
        disabled={!report && !problem} onClick={() => refresh()}><RefreshCw aria-hidden="true" /></button>
    </div>}
    {options && (options.keys.length > 0 || Object.values(selection).some(v => v !== undefined)) &&
      <UsageSelectors value={selection} options={options} onChange={select} />}
    {report && options || id ? <ReceiptPages key={id ?? selectionUrl} report={report} selection={selection}
      id={id} hours={hours} refresh={() => refresh()} /> : <ReportStatus error={problem} retry={retry} />}
  </section>;
}

function UsageSelectors({ value, options, onChange }: { value: UsageSelection; options: Options; onChange: (v: UsageSelection) => void }) {
  const { t } = useLocale();
  const fields = useRef<HTMLDivElement>(null);
  const chosen = Object.values(value).some(v => v !== undefined);
  return <div className="report-filter-set">
    <div className="report-usage-filters" ref={fields}>
      <UsageSelect label="common.model" value={value.model ?? ""} onChange={model => onChange({ ...value, model: model || undefined })}>
        <option value="">{t("usage.allModels")}</option>
        {value.model && !options.models.includes(value.model) && <option value={value.model}>{value.model}</option>}
        {options.models.map(m => <option key={m} value={m}>{m}</option>)}
      </UsageSelect>
      <UsageSelect label="common.apiKey" value={value.keyId ?? ""} onChange={keyId => onChange({ ...value, keyId: keyId || undefined })}>
        <option value="">{t("usage.allKeys")}</option>
        {value.keyId && !options.keys.some(k => k.id === value.keyId) && <option value={value.keyId}>{reportKeyPrefix(value.keyId)}</option>}
        {options.keys.map(k => <option key={k.id} value={k.id}>{k.name} · {k.prefix}</option>)}
      </UsageSelect>
      <UsageSelect label="report.outcome" value={value.execution ?? ""} onChange={v => onChange({ ...value, execution: isExecution(v) ? v : undefined })}>
        <option value="">{t("report.allOutcomes")}</option>
        {executions.map(v => <option key={v} value={v}>{t(outcomes[v])}</option>)}
      </UsageSelect>
    </div>
    {chosen && <button type="button" className="button report-reset" onClick={() => {
      fields.current?.querySelector("select")?.focus();
      onChange({});
    }}>{t("usage.resetFilters")}</button>}
  </div>;
}

function UsageSelect({ label, value, onChange, children }: { label: MessageKey; value: string;
  onChange: (v: string) => void; children: ReactNode }) {
  const { t } = useLocale();
  return <label className="catalog-select"><span className="sr-only">{t(label)}</span>
    <select value={value} onChange={e => onChange(e.currentTarget.value)}>{children}</select><ChevronDown aria-hidden="true" />
  </label>;
}

function ReceiptPages({ report, selection, id, hours, refresh }: { report: string | undefined; selection: UsageSelection;
  id: string | undefined; hours: Hours; refresh: () => void }) {
  const { t, locale } = useLocale();
  const [cursors, setCursors] = useState<string[]>([]);
  const [retry, setRetry] = useState(0);
  const before = cursors.at(-1);
  const filter = report ? { report, ...selection } : undefined;
  const load = useCallback((signal: AbortSignal) => readUsagePage(report ? { report, ...selection } : undefined, before, id, signal),
    [report, selection, before, id, retry]);
  const { data, error } = useReportRead(load);
  const fresh = usageLink(id ? {} : selection, hours);
  if (!data) return <ReportStatus error={error} retry={() => {
    if (error === "report_not_found" || error === "invalid_filter") {
      if (id) location.assign(fresh);
      else refresh();
    }
    else setRetry(x => x + 1);
  }} />;
  const window = data.window;
  return <>
    {window && <div className="report-window">
      <div className="report-window-heading">
        <div>
          <span title={localInterval(window.from, window.to, locale, true)}>{localInterval(window.from, window.to, locale)}</span>
          {window.to !== window.asOf && <small>{t("report.asOf", { time: new Intl.DateTimeFormat(locale, {
            timeZone: "UTC", dateStyle: "short", timeStyle: "medium",
          }).format(window.asOf) + " UTC" })}</small>}
        </div>
        {filter && !id && window.totals.requests > 0 && <ReportExport filter={filter} />}
      </div>
      <div className="report-totals">
        <dl><dt>{t("common.requests")}</dt><dd>{window.totals.requests.toLocaleString(locale)}</dd></dl>
        <dl><dt>{t("report.recordedTokens")}</dt><dd>{window.totals.tokens?.toLocaleString(locale) ?? "—"}</dd></dl>
        <dl><dt>{t("report.postedCharges")}</dt><dd>{formatMoney(window.totals.spend, 6, locale)} USD</dd></dl>
      </div>
      {(window.totals.samples < window.totals.requests || window.totals.partial > 0) &&
        <small>{t("report.tokenCoverage", { known: window.totals.samples.toLocaleString(locale),
          count: window.totals.requests.toLocaleString(locale), partial: window.totals.partial.toLocaleString(locale) })}</small>}
      {window.totals.unsettled > 0 && <small>{t("usage.unsettledCount", { count: window.totals.unsettled.toLocaleString(locale) })}</small>}
    </div>}
    {!data.items.length ? <p>{t("usage.noRequestsFound")}</p>
      : <ul className="own-key-list history-requests">{data.items.map(v => <li key={v.id}><RequestReceipt v={v}
        open={!!id} diagnostic={data.diagnostic} refresh={() => setRetry(s => s + 1)}
        liveLink={!id || report ? usageLink({}, hours) + "&id=" + v.id : undefined} /></li>)}</ul>}
    {(before || data.next) && <nav className="report-pager" aria-label={t("usage.usagePages")}>
      <button type="button" className="button" disabled={!before} onClick={() => setCursors(p => p.slice(0, -1))}>{t("pagination.previousPage")}</button>
      <button type="button" className="button" disabled={!data.next} onClick={() => { const next = data.next; if (next) setCursors(p => [...p, next]); }}>{t("pagination.nextPage")}</button>
    </nav>}
    {id && <a className="entry-account-link" href={window && filter ? usageLink(filter, window.hours) : fresh}>{t("nav.usage")}</a>}
  </>;
}

function RequestReceipt({ v, open, diagnostic, refresh, liveLink }: { v: Receipt; open: boolean;
  diagnostic: CustomerDiagnostic | null; refresh: () => void; liveLink: string | undefined }) {
  const { t, locale } = useLocale();
  const money = (n: bigint) => formatMoney(n, 6, locale) + " USD";
  const count = (n: bigint | null) => n?.toLocaleString(locale) ?? "—";
  const date = new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "medium", timeStyle: "medium" }).format(v.atMs) + " UTC";
  return <details className="own-receipt" open={open || undefined}>
    <summary>
      <span className="receipt-identity"><strong>{v.model}</strong><small>{t(outcomes[v.status])} · {t(accounting[v.accounting])}</small></span>
      <span className="receipt-amount">{v.charge === null ? "—" : money(v.charge - (v.credit?.amount ?? 0n))}</span>
    </summary>
    {diagnostic && <ReceiptDiagnostic event={diagnostic} receipt={v} refresh={refresh} />}
    <dl>
      <dt>{t("usage.requestIdentifier")}</dt><dd><code>{v.id}</code></dd>
      <dt>{t("receipt.started")}</dt><dd>{date}</dd>
      <dt>{t("common.apiKey")}</dt><dd>{reportKeyPrefix(v.keyId)}</dd>
      <dt>{t("usage.totalTokens")}</dt><dd>{count(v.total)}{v.quality === "partial" && <> · {t("receipt.partial")}</>}</dd>
      <dt>{t("usage.inputTokens")}</dt><dd>{count(v.input)}</dd>
      <dt>{t("usage.cacheRead")}</dt><dd>{count(v.cached)}</dd>
      <dt>{t("usage.outputTokens")}</dt><dd>{count(v.output)}</dd>
      <dt>{t("receipt.tariffRevision")}</dt><dd>{v.tariff.toLocaleString(locale)}</dd>
      <dt>{t("receipt.hold")}</dt><dd>{money(v.hold)}</dd>
      <dt>{t("common.charged")}</dt><dd>{v.charge === null ? t(accounting[v.accounting]) : money(v.charge)}{v.basis && <> · {t(basis[v.basis])}</>}</dd>
      {v.credit && <><dt>{t("receipt.credit")}</dt><dd><a className="entry-account-link"
        href={"/app/billing?id=" + v.credit.id}>+{money(v.credit.amount)}</a></dd></>}
    </dl>
    <div className="receipt-actions">
      {v.receipt && <a className="entry-account-link" href={"/app/billing?id=" + v.receipt}>{t("receipt.walletEntry")}</a>}
      {liveLink && <a className="entry-account-link" href={liveLink}>{t("request.requestDetails")}</a>}
    </div>
  </details>;
}

function ReceiptDiagnostic({ event, receipt, refresh }: { event: CustomerDiagnostic; receipt: Receipt; refresh: () => void }) {
  const { t } = useLocale();
  const support = JSON.stringify({
    api_version: 1, diagnostic_version: event.version, started_at: new Date(receipt.atMs).toISOString(), request_id: receipt.id,
    code: event.code, coverage: event.coverage, execution: receipt.status, accounting: receipt.accounting,
  }, null, 2);
  return <div className="receipt-diagnostic">
    <strong>{t(event.summary)}</strong>
    <code>{event.code}</code>
    <div className="receipt-actions">
      {event.action === "check_status"
        ? <button type="button" className="button" onClick={refresh}>{t("diagnostics.checkStatus")}</button>
        : <a className="entry-account-link" href="/legal">{t("public.legal")}</a>}
      <CopyButton label={t("diagnostics.copySupport")} text={support} />
    </div>
  </div>;
}
