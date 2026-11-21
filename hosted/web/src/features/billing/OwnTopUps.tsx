import { useEffect, useId, useRef, useState } from "react";
import { confirmTopUp, parseCreditInput, quoteTopUp, readCheckoutMethods, readTopUps, resumeTopUp } from "../../api/checkout";
import type { CheckoutMethod, CheckoutQuote, TopUp } from "../../api/checkout";
import { ControlError, uuidText } from "../../api/control";
import { RefreshCw } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { formatMoney } from "../usage/model";
import { OwnRefund } from "./OwnRefund";
import "./checkout.css";

type Pending = { quote: string; operation: string };
type Page = Awaited<ReturnType<typeof readTopUps>>;
type Problem = { key: MessageKey; count?: number; limit?: number };

function readPending(storage: string): Pending | null {
  try {
    const [quote, operation, extra] = (sessionStorage.getItem(storage) ?? "").split(":");
    if (quote && operation && extra === undefined && uuidText.test(quote) && uuidText.test(operation)) return { quote, operation };
  } catch { /* The list also recovers accepted operations. */ }
  return null;
}

const states = { processing: "checkout.processing", awaiting_payment: "checkout.awaiting",
  paid: "billing.credited", closed: "checkout.closed" } as const;

export function OwnTopUps({ csrf, workspace, onRefresh }: { csrf: string; workspace: string; onRefresh: () => void }) {
  const { t, locale } = useLocale();
  const storage = "relay.pendingTopUp." + workspace;
  const [pending, setPending] = useState(() => readPending(storage));
  const [methods, setMethods] = useState<CheckoutMethod[]>([]);
  const [page, setPage] = useState<Page>();
  const [methodId, setMethodId] = useState("");
  const method = methods.find(m => m.id === methodId) ?? methods[0];
  const [amount, setAmount] = useState("");
  const [review, setReview] = useState<CheckoutQuote & { label: string }>();
  const [problem, setProblem] = useState<Problem>();
  const [busy, setBusy] = useState(false);
  const [resuming, setResuming] = useState<string>();
  const locked = useRef(false);
  const heading = useRef<HTMLHeadingElement>(null), formHeading = useRef<HTMLHeadingElement>(null);
  const id = useId();
  const credit = parseCreditInput(amount);
  const valid = credit !== null && method && credit >= method.min && credit <= method.max &&
    (method.custom || method.quick.includes(credit));
  const money = (n: bigint) => formatMoney(n, 2, locale) + " USD";

  function remember(value: Pending | null) {
    setPending(value);
    try {
      if (value) sessionStorage.setItem(storage, value.quote + ":" + value.operation);
      else sessionStorage.removeItem(storage);
    } catch { /* Same-tab recovery still works. */ }
  }

  function fail(e: unknown) {
    if (e instanceof ControlError && e.status === 401) { location.replace("/login"); return; }
    setProblem({ key: "unavailable.server" });
  }

  async function lookup(value: Pending, signal?: AbortSignal) {
    const found = (await readTopUps({ operation_id: value.operation }, signal)).items[0];
    if (found && found.quote !== value.quote) throw new Error("Unexpected payment recovery");
    return found;
  }

  useEffect(() => {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort("timeout"), 12_000);
    let alive = true;
    const saved = readPending(storage);
    void Promise.all([readCheckoutMethods(controller.signal), readTopUps(undefined, controller.signal),
      saved ? lookup(saved, controller.signal) : null]).then(([m, history, found]) => {
      if (!alive) return;
      setMethods(m); setPage(history);
      if (found) accepted(found);
      else if (saved) setProblem({ key: "checkout.unconfirmed" });
    }).catch(e => { if (alive) fail(e); }).finally(() => clearTimeout(timer));
    return () => { alive = false; clearTimeout(timer); controller.abort(); };
  }, [storage]);

  async function act(fn: () => Promise<void>) {
    if (locked.current) return;
    locked.current = true; setBusy(true); setProblem(undefined);
    try { await fn(); } catch (e) { fail(e); }
    finally { locked.current = false; setBusy(false); }
  }

  async function refresh() {
    const [m, history] = await Promise.all([readCheckoutMethods(), readTopUps()]);
    setMethods(m); setPage(history); onRefresh();
    if (pending) {
      const found = await lookup(pending);
      if (found) accepted(found);
      else setProblem({ key: "checkout.unconfirmed" });
    }
  }

  function accepted(payment: TopUp) {
    remember(null); setReview(undefined); setAmount("");
    setPage(prev => {
      if (!prev) return { items: [payment], next: null };
      if (prev.items.some(p => p.id === payment.id))
        return { ...prev, items: prev.items.map(p => p.id === payment.id ? payment : p) };
      return { ...prev, items: [payment, ...prev.items] };
    });
    if (payment.receipt) onRefresh();
    requestAnimationFrame(() => heading.current?.focus());
  }

  async function resume(original: TopUp) {
    setResuming(original.id);
    try {
      const { payment, handoff } = await resumeTopUp(csrf, original);
      if (handoff) { location.assign(handoff); return; }
    updatePayment(payment);
      if (payment.receipt) onRefresh();
      if (payment.state === "awaiting_payment" || payment.state === "processing")
        setProblem({ key: "checkout.resumeUnavailable" });
      if (!payment.canResume) requestAnimationFrame(() => heading.current?.focus());
    } finally { setResuming(undefined); }
  }

  function updatePayment(payment: TopUp) {
    setPage(prev => prev && { ...prev, items: prev.items.map(p => p.id === payment.id ? payment : p) });
  }

  async function confirm(value: Pending) {
    remember(value);
    try { accepted(await confirmTopUp(csrf, value.quote, value.operation)); }
    catch (e) {
      if (e instanceof ControlError) {
        if (e.status === 401) throw e;
        if ([404, 410].includes(e.status) || e.code === "payment_method_unavailable") {
          remember(null); setReview(undefined);
          if (e.code === "payment_method_unavailable") setMethods(await readCheckoutMethods());
          setProblem({ key: e.code === "payment_method_unavailable" ? "checkout.methodUnavailable" : "checkout.quoteExpired" });
          requestAnimationFrame(() => formHeading.current?.focus()); return;
        }
        if (e.code === "open_invoice_limit") {
          const { open_count, open_limit } = e.details ?? {};
          remember(null);
          setProblem(typeof open_count === "number" && Number.isInteger(open_count) &&
            typeof open_limit === "number" && Number.isInteger(open_limit) && open_count >= open_limit && open_limit >= 1 && open_limit <= 10
            ? { key: "checkout.invoiceCount", count: open_count, limit: open_limit } : { key: "checkout.invoiceLimit" });
          setPage(await readTopUps()); return;
        }
        if (e.code === "checkout_conflict") {
          remember(null); setReview(undefined); setPage(await readTopUps()); setProblem({ key: "checkout.conflict" }); return;
        }
      }
      const found = await lookup(value);
      if (found) accepted(found);
      else setProblem({ key: "checkout.unconfirmed" });
    }
  }

  const date = (s: number) => new Intl.DateTimeFormat(locale, { timeZone: "UTC", dateStyle: "medium", timeStyle: "short" }).format(s * 1000) + " UTC";
  const visibleHistory = !!(page?.items.length || pending);
  if (!method && !visibleHistory && !problem) return null;
  return <section className="own-top-ups" aria-label={t(method ? "billing.topUpBalance" : "billing.paymentHistory")} aria-busy={busy}>
    {method && !pending && <div className="checkout-panel">
      <h2 ref={formHeading} tabIndex={-1}>{t(review ? "checkout.review" : "billing.topUpBalance")}</h2>
      {review ? <>
        <dl className="checkout-totals">
          <div><dt>{t("billing.paymentMethod")}</dt><dd>{review.label}</dd></div>
          <div><dt>{t("checkout.walletCredit")}</dt><dd>{money(review.credit)}</dd></div>
          {review.fee > 0n && <div><dt>{t("checkout.fee")}</dt><dd>{money(review.fee)}</dd></div>}
          <div className="checkout-payable"><dt>{t("checkout.total")}</dt><dd>{money(review.total)}</dd></div>
        </dl>
        <div className="checkout-actions">
          <button type="button" className="key-button key-secondary" disabled={busy} onClick={() => {
            setReview(undefined); setProblem(undefined); requestAnimationFrame(() => formHeading.current?.focus());
          }}>{t("common.cancel")}</button>
          <button type="button" className="key-button key-primary" disabled={busy}
            onKeyDown={e => { if (e.repeat && ["Enter", " "].includes(e.key)) e.preventDefault(); }}
            onClick={e => { if (e.detail < 2) void act(() => confirm({ quote: review.id, operation: crypto.randomUUID() })); }}>
            {t("checkout.confirm")}</button>
        </div>
      </> : <form className="checkout-form owner-setup-form" onSubmit={e => {
        e.preventDefault(); if (!valid || credit === null) return;
        void act(async () => {
          try {
            setReview({ ...await quoteTopUp(csrf, method.id, credit), label: method.label });
            requestAnimationFrame(() => formHeading.current?.focus());
          } catch (error) {
            if (error instanceof ControlError && error.code === "quote_limit") setProblem({ key: "checkout.quoteLimit" });
            else if (error instanceof ControlError && error.code === "payment_method_unavailable") {
              setMethods(await readCheckoutMethods()); setProblem({ key: "checkout.methodUnavailable" });
            } else throw error;
          }
        });
      }}>
        <div className="checkout-field">
          <span>{t("billing.paymentMethod")}</span>
          <OptionMenu label={t("billing.paymentMethod")} value={method.id} disabled={busy}
            options={methods.map(m => ({ value: m.id, label: m.label }))} onChange={v => {
              setMethodId(v); setAmount(""); setProblem(undefined);
            }} />
        </div>
        <div className="checkout-field">
          {method.custom ? <label htmlFor={id}>{t("billing.amountUsd")}
            <input id={id} inputMode="decimal" maxLength={8} autoComplete="off" disabled={busy}
              value={amount} aria-invalid={amount !== "" && !valid} onChange={e => setAmount(e.target.value)} />
          </label> : <span>{t("billing.amountUsd")}</span>}
          {!!method.quick.length && <div className="checkout-presets" role="group" aria-label={t("billing.quickAmount")}>
            {method.quick.map(n => <button type="button" className="button" key={String(n)} disabled={busy}
              aria-label={money(n)} aria-pressed={credit === n}
              onClick={() => setAmount(`${n / 1_000_000n}.${String((n / 10_000n) % 100n).padStart(2, "0")}`)}>
              {formatMoney(n, 2, locale)}</button>)}
          </div>}
          {method.custom && amount && !valid && <small>{t("checkout.amountRange", { min: money(method.min), max: money(method.max) })}</small>}
        </div>
        <button className="key-button key-primary" type="submit" disabled={busy || !valid}>{t("checkout.reviewAction")}</button>
      </form>}
    </div>}
    {pending && <div className="checkout-actions">
      <button type="button" className="key-button key-secondary" disabled={busy}
        onClick={() => void act(refresh)}>{t("checkout.checkStatus")}</button>
      <button type="button" className="key-button key-primary" disabled={busy}
        onClick={() => void act(() => confirm(pending))}>{t("checkout.retryConfirmation")}</button>
    </div>}
    {problem && <div className="checkout-problem" role="alert"><span>{t(problem.key, { count: problem.count ?? "", limit: problem.limit ?? "" })}</span>
      {problem.limit !== undefined && <a className="entry-account-link" href="#top-up-history">{t("billing.paymentHistory")}</a>}
      {!page && !pending && <button className="button" type="button" disabled={busy} onClick={() => void act(refresh)}>{t("common.retry")}</button>}
    </div>}
    {visibleHistory && <div className="checkout-history" id="top-up-history">
      <header><h2 ref={heading} tabIndex={-1}>{t("billing.paymentHistory")}</h2>
        <button className="button checkout-refresh" type="button" disabled={busy} aria-label={t("checkout.refresh")}
          title={t("checkout.refresh")} onClick={() => void act(refresh)}><RefreshCw aria-hidden="true" /></button></header>
      {!!page?.items.length && <ul className="own-key-list">{page.items.map(p => <li key={p.id}>
        <div className="own-key-detail"><strong>{p.label}</strong><span>{date(p.at)}</span>
          <span className="checkout-state" data-state={p.state}>{t(states[p.state])}</span></div>
        <div className="own-key-tools"><strong>{money(p.amount)}</strong>
          {p.total !== p.amount && <span>{t(p.state === "paid" ? "checkout.paidTotal" : "checkout.total")}: {money(p.total)}</span>}
          {p.canResume && <button className="key-button key-secondary checkout-continue" type="button"
            aria-disabled={busy} onClick={() => void act(() => resume(p))}>
            {t(resuming === p.id ? "checkout.checking" : "checkout.resume")}</button>}
          {p.receipt && <a className="entry-account-link checkout-receipt" href={"/app/billing?id=" + p.receipt}>{t("receipt.walletEntry")}</a>}</div>
        {p.receipt && <OwnRefund payment={p} csrf={csrf} disabled={busy} onUpdate={updatePayment} />}
      </li>)}</ul>}
      {page?.next && <button className="button" type="button" disabled={busy} onClick={() => void act(async () => {
        const more = await readTopUps({ before: page.next! });
        setPage({ items: [...page.items, ...more.items], next: more.next });
      })}>{t("pagination.nextPage")}</button>}
    </div>}
  </section>;
}
