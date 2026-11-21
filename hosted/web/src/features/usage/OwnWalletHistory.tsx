import { useCallback, useState } from "react";
import { uuidText } from "../../api/control";
import { readWalletHistory } from "../../api/history";
import type { Hours } from "../../api/usageReport";
import { useLocale } from "../../i18n/LocaleProvider";
import { ReportPeriods, ReportStatus, useReportRead } from "./ReportTools";
import { formatMoney } from "./model";

const entries = { payment_credit: "billing.credited", request_hold: "receipt.hold",
  request_close: "receipt.closed", request_credit: "receipt.credit", refund_hold: "refund.held",
  refund_paid: "billing.paymentRefund", refund_release: "refund.released" } as const;

export function OwnWalletHistory({ refresh = 0 }: { refresh?: number }) {
  const { t, locale } = useLocale();
  const [hours, setHours] = useState<Hours>(24), [before, setBefore] = useState<string>();
  const [retry, setRetry] = useState(0);
  const requested = new URLSearchParams(location.search).get("id");
  const valid = requested === null || uuidText.test(requested);
  const load = useCallback((signal: AbortSignal) =>
    readWalletHistory(hours, requested ? { id: requested } : before ? { before } : {}, signal), [hours, before, requested, retry, refresh]);
  const { data, error } = useReportRead(valid ? load : null);
  const date = (n: number) => new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" }).format(n * 1000) + " UTC";
  return <section className="own-history" aria-label={t("nav.balance")}>
    {!requested && <ReportPeriods hours={hours} onChange={h => { setBefore(undefined); setHours(h); }} />}
    {!data ? <ReportStatus error={valid ? error : "invalid_filter"}
      retry={valid ? () => setRetry(v => v + 1) : () => location.assign("/app/billing")} />
      : !data.items.length ? <p>{t("receipt.noEntries")}</p>
      : <ul className="own-key-list">{data.items.map(v => <li key={v.id}>
        <div className="own-key-detail"><strong>{t(entries[v.type])}</strong><span>{date(v.at)}</span>
          {v.requestId && <a className="entry-account-link" href={"/app/usage?id=" + v.requestId}>{t("usage.requestIdentifier")}</a>}</div>
        <span>{v.type === "request_credit" ? "+" : v.type === "refund_paid" ? "−" : ""}{formatMoney(v.amount, 6, locale)} USD</span>
      </li>)}</ul>}
    {data?.next && <button type="button" className="button" onClick={() => { if (data.next) setBefore(data.next); }}>{t("pagination.nextPage")}</button>}
    {(before || requested) && <a className="entry-account-link" href="/app/billing">{t("nav.balance")}</a>}
  </section>;
}
