import { useEffect, useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { readRefundCases, refundLabels, reviewRefund, type RefundAction } from "../../api/refunds";
import { RefreshCw } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { formatMoney } from "../usage/model";
import "../billing/refunds.css";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Review = { id: string; operation: string; action: RefundAction };
type Page = Awaited<ReturnType<typeof readRefundCases>>;
const problems: Record<string, MessageKey> = {
  refund_balance: "refund.insufficient", refund_conflict: "refund.changed", refund_remainder: "refund.remainderChanged",
  refund_closed: "refund.closed",
};

export function PaymentRefunds({ session, onSession }: { session: Session; onSession: (v: Session) => void }) {
  const { t, locale } = useLocale();
  const payment = new URLSearchParams(location.search).get("payment_id");
  const valid = payment === null || uuidText.test(payment);
  const [before, setBefore] = useState<string>();
  const [page, setPage] = useState<Page>();
  const [problem, setProblem] = useState<MessageKey>();
  const [confirm, setConfirm] = useState<Review>();
  const [busy, setBusy] = useState(false);
  const running = useRef(false), heading = useRef<HTMLHeadingElement>(null);

  function denied(err: unknown) {
    if (err instanceof ControlError && err.status === 401) { location.replace("/admin/login"); return true; }
    return false;
  }

  useEffect(() => {
    if (!valid) return;
    const c = new AbortController();
    void readRefundCases(payment, undefined, AbortSignal.any([c.signal, AbortSignal.timeout(12000)]))
      .then(v => { if (!c.signal.aborted) setPage(v); })
      .catch(err => { if (!c.signal.aborted && !denied(err)) setProblem("unavailable.server"); });
    return () => c.abort();
  }, [payment, valid]);

  async function run(fn: () => Promise<void>, applying = false) {
    if (running.current) return;
    running.current = true; setBusy(true); setProblem(undefined);
    try { await fn(); }
    catch (err) {
      if (denied(err)) return;
      const message = err instanceof ControlError && err.code ? problems[err.code] : undefined;
      setProblem(message ?? (applying ? "refund.reviewUnconfirmed" : "unavailable.server"));
    } finally { running.current = false; setBusy(false); }
  }

  async function load(cursor: string | undefined) {
    setPage(await readRefundCases(payment, cursor)); setBefore(cursor); setConfirm(undefined);
  }

  function apply(command: Review) {
    void run(async () => {
      const send = (csrf: string) => reviewRefund(csrf, command.id, command.operation, command.action);
      try { await send(session.csrf); }
      catch (error) {
        if (!(error instanceof ControlError) || error.code !== "fresh_passkey_required") throw error;
        try {
          await signInOwner();
        } catch {
          setProblem("paymentBuilder.verifyFailed"); return;
        }
        const renewed = await readOwnerOverview(AbortSignal.timeout(10000));
        onSession(renewed);
        await send(renewed.csrf);
      }
      await load(before);
      requestAnimationFrame(() => heading.current?.focus());
    }, true);
  }

  const date = new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" });
  return <section className="owner-refunds" aria-label={t("refund.title")} aria-busy={busy || (!page && !problem && valid)}>
    <div className="owner-refund-header">
      <h2 ref={heading} tabIndex={-1}>{t("refund.requests")}</h2>
      {valid && <button type="button" className="button refund-refresh" aria-label={t("refund.refresh")} title={t("refund.refresh")}
        disabled={busy || (!page && !problem)} onClick={() => void run(() => load(before))}><RefreshCw aria-hidden="true" /></button>}
    </div>
    {!valid && <p role="alert">{t("refund.invalidPayment")}</p>}
    {payment !== null && <a className="entry-account-link" href="/admin/payments">{t("refund.all")}</a>}
    {problem && <span className="refund-problem" role="alert">{t(problem)}</span>}
    {!page && valid && !problem && <p role="status">{t("unavailable.connecting")}</p>}
    {page && !page.items.length && <p>{t("refund.empty")}</p>}
    {!!page?.items.length && <ul className="own-key-list">{page.items.map(r => {
      const amount = `${formatMoney(r.amount, 2, locale)} USD`;
      const selected = confirm?.id === r.id ? confirm : undefined;
      const start = (action: RefundAction) => {
        setProblem(undefined); setConfirm({ id: r.id, action, operation: crypto.randomUUID() });
      };
      return <li key={r.id}>
        <div className="own-key-detail">
          <strong>{r.label}</strong>
          <span>{date.format(r.at * 1000)} UTC</span>
          <a className="entry-account-link refund-id" href={"/admin/payments?payment_id=" + r.paymentId}>{r.paymentId}</a>
        </div>
        <div className="own-key-tools">
          <strong>{amount}</strong>
          <div className="refund-state" data-state={r.state}>{t(refundLabels[r.state])}</div>
        </div>
        {r.state === "requested" && <div className="owner-refund-confirm">
          {selected ? <>
            <p>{t(selected.action === "approve" ? "refund.approveQuestion" : "refund.declineQuestion", { amount })}</p>
            <div className="refund-actions">
              <button type="button" autoFocus className="key-button key-secondary" disabled={busy} onClick={() => {
                setConfirm(undefined); setProblem(undefined);
                requestAnimationFrame(() => document.getElementById(`${r.id}-${selected.action}`)?.focus());
              }}>{t("common.cancel")}</button>
              <button type="button" className={`key-button ${selected.action === "approve" ? "key-primary" : "key-secondary"}`}
                disabled={busy} onClick={() => apply(selected)}>
                {t(selected.action === "approve" ? "refund.confirmApprove" : "refund.confirmDecline")}</button>
            </div>
          </> : <div className="refund-actions">
            <button id={`${r.id}-approve`} type="button" className="key-button key-secondary" disabled={busy}
              onClick={() => start("approve")}>{t("refund.approve")}</button>
            <button id={`${r.id}-decline`} type="button" className="button" disabled={busy}
              onClick={() => start("decline")}>{t("refund.decline")}</button>
          </div>}
        </div>}
      </li>;
    })}</ul>}
    {(before || page?.next) && <div className="refund-actions refund-pager">
      {before && <button type="button" className="button" disabled={busy}
        onClick={() => void run(() => load(undefined))}>{t("refund.newest")}</button>}
      {page?.next && <button type="button" className="button" disabled={busy}
        onClick={() => void run(() => load(page.next!))}>{t("pagination.nextPage")}</button>}
    </div>}
  </section>;
}
