import { useCallback, useEffect, useRef, useState } from "react";
import { defaultOwnerUsage, ownerRequestPath, ownerUsageHref, parseOwnerUsageView, readOwnerRequest } from "../../api/ownerUsage";
import { customerCasePath } from "../../api/customerAccess";
import { diagnosticOrigins, diagnosticSeverities, diagnosticStages, type OwnerDiagnostic } from "../../api/requestDiagnostics";
import { ArrowRight, Check, Copy, RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { reportKeyPrefix } from "../../api/usageReport";
import { formatMoney } from "../usage/model";
import { ReportStatus, useReportRead } from "../usage/ReportTools";
import { OwnerRequestStates } from "./OwnerUsage";
import { executionLabel, issueLabel, usd, utcStamp } from "./ownerUsageDisplay";

const settlementBasis = { provider_final: "receipt.final", provider_partial: "receipt.partial",
  not_sent: "receipt.notSent", policy_waiver: "receipt.waived" } as const;
const eventLabel = { reserved: "ownerUsage.reservation", dispatch_permitted: "ownerUsage.permit",
  terminal_observed: "ownerUsage.terminal", terminal_applied: "ownerUsage.terminalApplied", hold_closed: "ownerUsage.holdClosed",
  late_usage: "ownerUsage.lateUsage", credit_posted: "ownerUsage.creditPosted" } as const;

export function OwnerRequest() {
  const { t, locale } = useLocale();
  const view = parseOwnerUsageView(location.search) ?? defaultOwnerUsage;
  const id = ownerRequestPath(location.pathname);
  const [seq, setSeq] = useState(0);
  const read = useCallback((signal: AbortSignal) => readOwnerRequest(id!, signal), [id, seq]);
  const { data, error } = useReportRead(id ? read : null, "/admin/login");
  return <section className="owner-usage owner-case" aria-label={t("ownerUsage.detail")}>
    <div className="owner-usage-bar">
      <a className="owner-back-link entry-account-link" href={ownerUsageHref(view)}><ArrowRight aria-hidden="true" />{t("ownerUsage.back")}</a>
      <button className="icon-button report-refresh" type="button" title={t("ownerUsage.refreshRequest")} aria-label={t("ownerUsage.refreshRequest")}
        aria-disabled={!id || (!data && !error) || error === "not_found"} onClick={() => {
          if (id && (data || error && error !== "not_found")) setSeq(n => n + 1);
        }}><RefreshCw aria-hidden="true" /></button>
    </div>
    {!id || error === "not_found" ? <div className="owner-usage-empty" role="status"><p>{t("ownerUsage.missing")}</p></div>
      : !data ? <div className="owner-usage-empty"><ReportStatus error={error} retry={() => setSeq(n => n + 1)} /></div>
      : <>
        <div className="owner-case-header">
          <div><h2>{data.receipt.model}</h2>
            <OwnerRequestStates execution={data.receipt.status} accounting={data.receipt.accounting} />
          </div>
          <time dateTime={new Date(data.asOf).toISOString()}>{t("report.asOf", { time: utcStamp(data.asOf, locale) })}</time>
        </div>
        <CaseReceipt data={data} />
        {(data.diagnostics.length > 0 || data.attention.includes("execution")) && <DiagnosticObservations events={data.diagnostics} />}
        <div className="owner-case-body">
          <section aria-labelledby="owner-timeline">
            <h3 id="owner-timeline">{t("ownerUsage.timeline")}</h3>
            <ol className="owner-timeline">
              {data.timeline.map(({ event, at }) => <li key={event}>
                <div className="owner-timeline-point"><span>{t(eventLabel[event])}</span>
                  <time dateTime={new Date(at).toISOString()}>{utcStamp(at, locale, true)}</time></div>
                {event === "dispatch_permitted" && data.attempt && <dl className="owner-attempt">
                  <dt>{t("ownerUsage.attemptId")}</dt><dd><code>{data.attempt.id}</code></dd>
                  <dt>{t("ownerUsage.sourceId")}</dt><dd>{data.attempt.source
                    ? <a className="entry-account-link" href={ownerUsageHref({ ...view, source: data.attempt.source })}><code>{data.attempt.source}</code></a> : "—"}</dd>
                  {data.attempt.revision !== null && <><dt>{t("ownerUsage.purchaseRevision")}</dt><dd>{data.attempt.revision.toLocaleString(locale)}</dd></>}
                  <dt>{t("ownerUsage.supplierValue")}</dt><dd>{usd(data.attempt.value, locale)}</dd>
                  {data.attempt.terminal && <><dt>{t("ownerUsage.terminalState")}</dt><dd>{t(executionLabel[data.attempt.terminal])}</dd></>}
                </dl>}
              </li>)}
            </ol>
            {data.attempt && <p className="owner-case-note">{t("ownerUsage.deliveryUnknown")}</p>}
          </section>
          <section aria-labelledby="owner-context">
            <h3 id="owner-context">{t("ownerUsage.context")}</h3>
            <dl className="owner-case-context">
              <dt>{t("ownerUsage.requestId")}</dt><dd><CopyReference id={data.receipt.id} /></dd>
              <dt>{t("ownerUsage.workspaceId")}</dt><dd><a className="entry-account-link" href={customerCasePath(data.workspace)}
                aria-label={t("access.openCase", { id: data.workspace })}><code>{data.workspace}</code></a></dd>
              <dt>{t("common.apiKey")}</dt><dd><a className="entry-account-link" href={ownerUsageHref({ ...view, key: data.receipt.keyId })}><code>{reportKeyPrefix(data.receipt.keyId)}</code></a></dd>
              <dt>{t("usage.protocolTransport")}</dt><dd>Responses · {data.streaming ? "SSE" : "JSON"}</dd>
              <dt>{t("receipt.tariffRevision")}</dt><dd>{data.receipt.tariff.toLocaleString(locale)}</dd>
              <dt>{t("usage.inputTokens")}</dt><dd>{data.receipt.input?.toLocaleString(locale) ?? "—"}</dd>
              <dt>{t("usage.cacheRead")}</dt><dd>{data.receipt.cached?.toLocaleString(locale) ?? "—"}</dd>
              <dt>{t("usage.outputTokens")}</dt><dd>{data.receipt.output?.toLocaleString(locale) ?? "—"}</dd>
            </dl>
          </section>
        </div>
      </>}
  </section>;
}

function DiagnosticObservations({ events }: { events: OwnerDiagnostic[] }) {
  const { t, locale } = useLocale();
  return <section className="owner-diagnostics" aria-labelledby="owner-diagnostic-heading">
    <div className="owner-diagnostics-heading">
      <h3 id="owner-diagnostic-heading">{t("diagnostics.title")}</h3>
      <span>{t(events.length ? "diagnostics.partialCoverage" : "ownerUsage.causeUnknown")}</span>
    </div>
    {events.length > 0 && <ul className="owner-timeline">
      {events.map(event => <li key={event.id}>
        <div className="owner-timeline-point">
          <strong>{t(event.summary)}</strong>
          <time dateTime={new Date(event.happened ?? event.observed).toISOString()} title={utcStamp(event.observed, locale, true)}>
            {event.happened === null && <>{t("diagnostics.recorded")} </>}{utcStamp(event.happened ?? event.observed, locale, true)}
          </time>
        </div>
        <div className="owner-diagnostic-meta">
          <span>{t(diagnosticStages[event.stage])}</span>
          <span data-severity={event.severity}>{t(diagnosticSeverities[event.severity])}</span>
          {event.origin && <span>{t(diagnosticOrigins[event.origin])}</span>}
          <code>{event.code}</code>
          {event.status !== null && <span>HTTP {event.status}</span>}
          {event.causeUnknown && <span>{t("ownerUsage.causeUnknown")}</span>}
        </div>
      </li>)}
    </ul>}
  </section>;
}

function CaseReceipt({ data }: { data: Awaited<ReturnType<typeof readOwnerRequest>> }) {
  const { t } = useLocale();
  const { hold, charge, credit, basis, quality } = data.receipt;
  const context = [basis && t(settlementBasis[basis]), quality === "final" && basis !== "provider_final" && t("receipt.final"),
    !basis && quality === "partial" && t("receipt.partial")].filter(Boolean).join(" · ");
  const attention = data.attention.filter(issue => issue !== "execution" && (issue !== "settlement" || data.receipt.accounting !== "pending"));
  return <div className="owner-case-money">
    <dl>
      <div><dt>{t("overview.apiCharges")}</dt><dd className="owner-case-net"><CaseMoney amount={charge === null ? null : charge - (credit?.amount ?? 0n)} /></dd></div>
      <div><dt>{t("ownerUsage.initialHold")}</dt><dd><CaseMoney amount={hold} /></dd></div>
      {credit && <><div><dt>{t("ownerUsage.originalCharge")}</dt><dd><CaseMoney amount={charge} /></dd></div>
        <div><dt>{t("receipt.credit")}</dt><dd><CaseMoney amount={credit.amount} /></dd></div></>}
    </dl>
    {context && <p className="owner-case-note">{context}</p>}
    {attention.length > 0 && <ul>{attention.map(issue => <li key={issue}>{t(issueLabel[issue])}</li>)}</ul>}
  </div>;
}

function CaseMoney({ amount }: { amount: bigint | null }) {
  const { locale } = useLocale();
  return amount === null ? "—" : <>{formatMoney(amount, 6, locale)}{" "}<span className="owner-case-currency">USD</span></>;
}

function CopyReference({ id }: { id: string }) {
  const { t } = useLocale();
  const [state, setState] = useState<"copied" | "failed">();
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return <div className="owner-copy-reference">
    <code>{id}</code>
    <button className="icon-button" type="button" aria-label={t("ownerUsage.copyId")} title={t("ownerUsage.copyId")} onClick={async () => {
      window.clearTimeout(timer.current);
      try {
        await navigator.clipboard.writeText(id);
        setState("copied");
        timer.current = window.setTimeout(() => setState(undefined), 1500);
      } catch { setState("failed"); }
    }}>{state === "copied" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}</button>
    <span className={state === "failed" ? "owner-case-note" : "sr-only"} role="status">
      {state ? t(state === "copied" ? "common.copied" : "common.copyUnavailable") : ""}
    </span>
  </div>;
}
