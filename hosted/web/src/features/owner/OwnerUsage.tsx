import { useCallback, useState, type FormEvent } from "react";
import { uuidText } from "../../api/control";
import { accountings, defaultOwnerUsage, hasDiagnosticSelection, ownerUsageHref, parseOwnerUsageView, readOwnerUsagePage, type OwnerUsageView } from "../../api/ownerUsage";
import { diagnosticChoices, diagnosticStages } from "../../api/requestDiagnostics";
import { executions } from "../../api/usageReport";
import { ChevronDown, ChevronRight, RefreshCw, Search } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { localInterval, ReportPeriods, ReportStatus, useReportRead } from "../usage/ReportTools";
import { accountingLabel, executionLabel, issueLabel, usd, utcStamp } from "./ownerUsageDisplay";
import "./owner-usage.css";

export function OwnerUsage() {
  const { t } = useLocale();
  const [view] = useState(() => parseOwnerUsageView(location.search));
  if (!view) return <div className="owner-usage-empty" role="alert">
    <p>{t("ownerUsage.invalid")}</p><a className="button" href="/admin/usage">{t("usage.resetFilters")}</a>
  </div>;
  return <UsageFeed view={view} />;
}

function UsageFeed({ view }: { view: OwnerUsageView }) {
  const { t, locale } = useLocale();
  const [pages, setPages] = useState<{ stack: string[]; window: OwnerUsageView["window"] }>({ stack: [], window: view.window });
  const [retry, setRetry] = useState(0);
  const before = pages.stack.at(-1);
  const load = useCallback((signal: AbortSignal) => readOwnerUsagePage(view, pages.window, before, signal), [view, pages.window, before, retry]);
  const { data, error, lastData } = useReportRead(load, "/admin/login");
  const loading = !data && !error;
  const scope = data ? { ...view, window: { from: data.from, to: data.to } } : view;
  const choose = (v: OwnerUsageView) => location.assign(ownerUsageHref(v));
  const refresh = () => { setPages({ stack: [], window: view.window }); setRetry(n => n + 1); };
  const diagnosticFiltered = hasDiagnosticSelection(view);
  const filterCount = [view.model, view.workspace, view.key, view.source, view.execution, view.accounting,
    view.stage, view.origin, view.category, view.severity, view.code].filter(Boolean).length;
  return <section className="owner-usage" aria-label={t("nav.usage")}>
    <div className="owner-usage-bar">
      <nav className="owner-usage-tabs" aria-label={t("usage.usageView")}>
        {[false, true].map(errors => <a key={String(errors)} href={ownerUsageHref({ ...view, errors })}
          className="button" aria-current={view.errors === errors ? "page" : undefined}>{t(errors ? "common.errors" : "common.requests")}</a>)}
      </nav>
      <RequestLookup view={scope} />
    </div>
    <div className="owner-usage-controls">
      <ReportPeriods hours={"hours" in view.window ? view.window.hours : undefined} onChange={hours => choose({ ...view, window: { hours } })} />
      <button type="button" className="icon-button report-refresh" aria-label={t("usage.refreshUsage")}
        title={t("usage.refreshUsage")} aria-disabled={loading} onClick={() => { if (!loading) refresh(); }}><RefreshCw aria-hidden="true" /></button>
    </div>
    <UsageFilters view={view} count={filterCount} choose={choose} />
    <div className="owner-usage-snapshot" aria-live="polite">
      {data && <><span>{localInterval(data.from, data.to, locale, true)}</span>
        <time dateTime={new Date(data.asOf).toISOString()}>{t("report.asOf", { time: utcStamp(data.asOf, locale) })}</time></>}
    </div>
    {data ? data.items.length ?
      <ul className="owner-request-list" aria-label={t(view.errors ? "common.errors" : "common.requests")}>
        {data.items.map(r => <li key={r.id}>
          <a className="owner-request-row" href={ownerUsageHref(scope, r.id)}>
            <span className="sr-only">{t("usage.requestDetailsId", { id: r.id })}</span>
            <div className="owner-request-identity"><strong>{r.model}</strong>
              <time dateTime={new Date(r.at).toISOString()}>{utcStamp(r.at, locale)}</time></div>
            <div className="owner-request-status">
              <OwnerRequestStates execution={r.execution} accounting={r.accounting} />
              {r.diagnostic && (r.issues.length > 0 || diagnosticFiltered) &&
                <span className="owner-request-diagnostic">{t(diagnosticStages[r.diagnostic.stage])} · {t(r.diagnostic.summary)}</span>}
              {r.issues.filter(x => x !== "settlement" && x !== "execution").map(x => <small key={x}>{t(issueLabel[x])}</small>)}
            </div>
            <span className="owner-request-amount"><span className="sr-only">{t("overview.apiCharges")}: </span>{usd(r.posted, locale)}</span>
            <ChevronRight aria-hidden="true" />
          </a>
        </li>)}
      </ul>
      : <div className="owner-usage-empty"><p>{t(view.errors ? "ownerUsage.noErrors" : "usage.noRequestsFound")}</p></div>
      : error === "invalid_filter" ? <div className="owner-usage-empty" role="alert"><p>{t("ownerUsage.invalid")}</p>
        <a className="button" href="/admin/usage">{t("usage.resetFilters")}</a></div>
      : error === "cursor_unavailable" ? <div className="owner-usage-empty" role="alert"><p>{t("ownerUsage.cursorUnavailable")}</p>
        <button className="button" onClick={refresh}>{t("usage.refreshUsage")}</button></div>
      : <div className="owner-usage-empty"><ReportStatus error={error} retry={() => setRetry(n => n + 1)} /></div>}
    {(before || lastData?.next) && <nav className="report-pager" aria-label={t("usage.usagePages")}>
      <button className="button" type="button" aria-disabled={loading || !before} onClick={() => {
        if (!loading && before) setPages(p => ({ ...p, stack: p.stack.slice(0, -1) }));
      }}>{t("pagination.previousPage")}</button>
      <button className="button" type="button" aria-disabled={!data?.next} onClick={() => {
        if (data?.next) setPages({ stack: [...pages.stack, data.next], window: { from: data.from, to: data.to } });
      }}>{t("pagination.nextPage")}</button>
    </nav>}
  </section>;
}

function RequestLookup({ view }: { view: OwnerUsageView }) {
  const { t } = useLocale();
  const [invalid, setInvalid] = useState(false);
  function lookup(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const field = e.currentTarget.elements.namedItem("request") as HTMLInputElement;
    const id = field.value.trim().toLowerCase();
    if (!uuidText.test(id)) { setInvalid(true); field.focus(); }
    else location.assign(ownerUsageHref(view, id));
  }
  return <form className="owner-request-lookup" onSubmit={lookup}>
    <label><span className="sr-only">{t("ownerUsage.requestId")}</span>
      <input name="request" type="search" placeholder={t("ownerUsage.requestId")} maxLength={48} autoComplete="off"
        spellCheck={false} aria-invalid={invalid || undefined} aria-describedby={invalid ? "owner-lookup-error" : undefined}
        onChange={() => setInvalid(false)} /></label>
    <button className="icon-button" type="submit" aria-label={t("ownerUsage.findRequest")} title={t("ownerUsage.findRequest")}><Search aria-hidden="true" /></button>
    {invalid && <small id="owner-lookup-error" role="alert">{t("ownerUsage.invalidId")}</small>}
  </form>;
}

function UsageFilters({ view, count, choose }: { view: OwnerUsageView; count: number; choose: (v: OwnerUsageView) => void }) {
  const { t, locale } = useLocale();
  const [invalid, setInvalid] = useState(false);
  const inputs = [["model", "common.model", view.model, 128], ["workspace_id", "ownerUsage.workspaceId", view.workspace, 48],
    ["key_id", "ownerUsage.keyId", view.key, 48], ["source_id", "ownerUsage.sourceId", view.source, 48]] as const;
  function apply(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const params = new URL(ownerUsageHref({ ...defaultOwnerUsage, window: view.window, errors: view.errors }), location.origin).searchParams;
    for (const [field, entry] of new FormData(e.currentTarget)) {
      const value = String(entry).trim();
      if (value) params.set(field, field.endsWith("_id") ? value.toLowerCase() : value);
    }
    const next = parseOwnerUsageView(params.toString());
    if (next) choose(next); else setInvalid(true);
  }
  return <details className="owner-usage-filters">
    <summary><span>{t("usage.additionalFilters")}</span>
      {count > 0 && <small>({count.toLocaleString(locale)})</small>}
      <ChevronDown aria-hidden="true" />
    </summary>
    <form onSubmit={apply} onChange={() => setInvalid(false)}>
      <div className="owner-filter-grid">
        {inputs.map(([name, label, value, max]) => <label key={name}>{t(label)}
          <input name={name} defaultValue={value} maxLength={max} autoComplete="off" spellCheck={false} />
        </label>)}
        <label>{t("usage.execution")}<select name="execution" defaultValue={view.execution ?? ""}><option value="">{t("usage.any")}</option>
          {executions.map(v => <option value={v} key={v}>{t(executionLabel[v])}</option>)}</select></label>
        <label>{t("usage.accounting")}<select name="accounting" defaultValue={view.accounting ?? ""}><option value="">{t("usage.any")}</option>
          {accountings.map(v => <option value={v} key={v}>{t(accountingLabel[v])}</option>)}</select></label>
      </div>
      {(view.errors || hasDiagnosticSelection(view)) && <fieldset className="owner-diagnostic-filters">
        <legend>{t("diagnostics.title")}</legend>
        <div className="owner-filter-grid">
          {Object.entries(diagnosticChoices).map(([field, options]) => {
            const name = field as keyof typeof diagnosticChoices;
            return <label key={field}>{t(diagnosticFilterLabels[name])}
              <select name={field} defaultValue={view[name] ?? ""}>
                <option value="">{t("usage.any")}</option>
                {Object.entries(options).map(([value, message]) => <option value={value} key={value}>{t(message)}</option>)}
              </select>
            </label>;
          })}
          <label>{t("common.errorCode")}<input name="code" defaultValue={view.code} maxLength={64} autoComplete="off" spellCheck={false} /></label>
        </div>
      </fieldset>}
      {invalid && <small role="alert">{t("ownerUsage.invalid")}</small>}
      <div className="owner-filter-actions"><button type="submit" className="key-button key-secondary">{t("ownerUsage.applyFilters")}</button>
        {count > 0 && <a className="button" href={ownerUsageHref({ ...defaultOwnerUsage, window: view.window, errors: view.errors })}>{t("usage.resetFilters")}</a>}</div>
    </form>
  </details>;
}

const diagnosticFilterLabels = {
  stage: "diagnostics.stage", origin: "diagnostics.origin", category: "diagnostics.category", severity: "diagnostics.severity",
} as const;

export function OwnerRequestStates({ execution, accounting }: { execution: keyof typeof executionLabel; accounting: keyof typeof accountingLabel }) {
  const { t } = useLocale();
  return <div className="owner-request-states">
    <span data-execution={execution}><span className="sr-only">{t("usage.execution")}: </span>{t(executionLabel[execution])}</span>
    <span className="owner-accounting"><span className="sr-only">{t("usage.accounting")}: </span>{t(accountingLabel[accounting])}</span>
  </div>;
}
