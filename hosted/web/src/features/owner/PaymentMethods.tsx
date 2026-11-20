import { useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { changePaymentBuilder, checkoutSettings, readPaymentBuilder, type BuilderAction,
  type BuilderCommand, type PaymentBuilder } from "../../api/paymentBuilder";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { CheckoutDraftFields, toFields, type DraftFields } from "./CheckoutDraftFields";
import "./payment-methods.css";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Confirmation = Extract<BuilderAction, { action: "publish" | "disable" }>;

export function PaymentMethods({ session, onSession }: { session: Session; onSession: (s: Session) => void }) {
  const { t, locale } = useLocale();
  const [view, setView] = useState<PaymentBuilder>();
  const [fields, setFields] = useState<DraftFields>();
  const [dirty, setDirty] = useState(false);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [fault, setFault] = useState<MessageKey>();
  const [status, setStatus] = useState<MessageKey>();
  const [retry, setRetry] = useState<BuilderCommand>();
  const [confirming, setConfirming] = useState<Confirmation>();
  const running = useRef(false);
  const returnFocus = useRef("");
  const publishedHeading = useRef<HTMLHeadingElement>(null);
  const draftHeading = useRef<HTMLHeadingElement>(null);

  function apply(current: PaymentBuilder) {
    setView(current); setFields(toFields(current.draft)); setDirty(false);
  }
  useEffect(() => {
    const abort = new AbortController();
    void readPaymentBuilder(AbortSignal.any([abort.signal, AbortSignal.timeout(10000)])).then(current => {
      if (!abort.signal.aborted) apply(current);
    }, error => {
      if (abort.signal.aborted) return;
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else setFault("unavailable.server");
    });
    return () => abort.abort();
  }, [reload]);

  function refresh() {
    setView(undefined); setFault(undefined); setStatus(undefined); setRetry(undefined); setConfirming(undefined);
    setReload(n => n + 1);
  }

  async function run(command: BuilderCommand) {
    if (running.current) return;
    running.current = true;
    setBusy(true); setFault(undefined); setStatus(undefined);
    let newPasskey = false;
    try {
      try {
        await changePaymentBuilder(session.csrf, command);
      } catch (error) {
        if (!(error instanceof ControlError) || error.code !== "fresh_passkey_required") throw error;
        newPasskey = true;
        await signInOwner();
        const renewed = await readOwnerOverview(AbortSignal.timeout(10000));
        onSession(renewed); newPasskey = false;
        await changePaymentBuilder(renewed.csrf, command);
      }
      apply(await readPaymentBuilder());
      setRetry(undefined); setConfirming(undefined);
      const action = command.command.action;
      if (action !== "check") setStatus(action === "save" ? "paymentBuilder.saved"
        : action === "publish" ? "paymentBuilder.applied" : "paymentBuilder.disabled");
      if (retry || action === "disable" || action === "publish") {
        requestAnimationFrame(() => (action === "disable" ? publishedHeading : draftHeading).current?.focus());
      }
    } catch (error) {
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else if (newPasskey) { setFault("paymentBuilder.verifyFailed"); }
      else if (error instanceof ControlError && (error.code === "revision_conflict" || error.code === "checkout_check_required"
          || error.code === "invalid_checkout_configuration" || error.code === "checkout_closed")) {
        setRetry(undefined); setConfirming(undefined);
        if (error.code === "checkout_check_required") setView(v => v && { ...v, eligibleCount: null, checkedUntil: null });
        setFault(error.code === "revision_conflict" ? "paymentBuilder.changed" : error.code === "checkout_check_required"
          ? "paymentBuilder.checkAgain" : error.code === "checkout_closed" ? "paymentBuilder.closed" : "paymentBuilder.invalid");
      } else { setRetry(command); setFault("paymentBuilder.actionFailed"); }
    } finally { running.current = false; setBusy(false); }
  }

  function submit(command: BuilderAction) {
    if (view) void run({ operation_id: crypto.randomUUID(), expected_revision: view.revision, command });
  }

  if (!view || !fields) return <section className="payment-builder">
    {fault ? <><p role="alert">{t(fault)}</p><button type="button" className="key-button key-secondary" onClick={refresh}>{t("common.retry")}</button></>
      : <p role="status">{t("unavailable.connecting")}</p>}
  </section>;
  if (!view.editable) return <p>{t("paymentBuilder.closed")}</p>;

  const usd = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" });
  const settings = checkoutSettings(fields.minimum, fields.quick, fields.custom_amount, fields.limit);
  const checked = view.eligibleCount !== null && (view.checkedUntil ?? 0) * 1000 > Date.now();
  const stale = fault === "paymentBuilder.changed";
  const locked = busy || !!retry || stale;
  const confirmButtons = (target: Confirmation) => <div className="payment-confirm">
    <p>{t(target.action === "publish" ? "paymentBuilder.publishQuestion" : "paymentBuilder.disableQuestion")}</p>
    <div className="payment-actions">
      <button autoFocus type="button" className="key-button key-secondary" disabled={locked}
        onClick={() => {
          setConfirming(undefined); setFault(undefined);
          requestAnimationFrame(() => document.getElementById(returnFocus.current)?.focus());
        }}>{t("common.cancel")}</button>
      <button type="button" className={`key-button ${target.action === "publish" ? "key-primary" : "key-secondary payment-danger"}`}
        disabled={locked} onClick={() => submit(target)}>
        {t(target.action === "publish" ? "paymentBuilder.confirmPublish" : "paymentBuilder.confirmDisable")}
      </button>
    </div>
  </div>;

  return <div className="payment-builder" aria-busy={busy || undefined}>
    <section className="payment-builder-panel" aria-labelledby="published-payment-title">
      <header className="payment-panel-heading"><h2 id="published-payment-title" tabIndex={-1} ref={publishedHeading}>{t("paymentBuilder.published")}</h2>
        {view.published.length > 0 && <span className="payment-caption">{t("paymentBuilder.liveLimit", { count: view.openLimit })}</span>}</header>
      {view.published.length === 0 ? <p className="payment-caption">{t("paymentBuilder.noPublished")}</p>
        : <ol className="payment-list" role="list" aria-labelledby="published-payment-title">
          {view.published.map(m => <li key={m.id}>
            <div className="payment-name"><strong>{m.label}</strong><small>{t(m.payer === "merchant" ? "paymentBuilder.merchant" : "paymentBuilder.customer")}
              {" · "}{t("paymentBuilder.fee", { amount: usd.format(m.fee / 100) })}</small></div>
            {confirming?.action === "disable" && confirming.method_id === m.id ? confirmButtons(confirming)
              : <button type="button" id={`disable-payment-${m.id}`} className="text-button payment-danger" disabled={locked || !!confirming || dirty}
                aria-label={t("paymentBuilder.disableItem", { name: m.label })} onClick={e => {
                  returnFocus.current = e.currentTarget.id;
                  setFault(undefined); setStatus(undefined); setConfirming({ action: "disable", method_id: m.id });
                }}>{t("paymentBuilder.disable")}</button>}
          </li>)}
        </ol>}
    </section>
    <section className="payment-builder-panel" aria-labelledby="checkout-draft-title">
      <header className="payment-panel-heading"><h2 id="checkout-draft-title" tabIndex={-1} ref={draftHeading}>{t("paymentBuilder.draft")}</h2>
        <span className="payment-sandbox">{t("paymentBuilder.sandbox")}</span></header>
      <form className="payment-editor owner-setup-form" onSubmit={e => {
        e.preventDefault();
        if (dirty && settings && !locked && !confirming)
          submit({ action: "save", draft: { ...settings, connections: fields.connections, methods: fields.methods } });
      }}>
        <fieldset disabled={locked || !!confirming}>
          <CheckoutDraftFields adapters={view.adapters} fields={fields} noAmounts={dirty ? [] : view.noAmounts} changed={next => {
            setFields(next); setDirty(true); setStatus(undefined); setFault(undefined);
          }} />
        </fieldset>
        <div className="payment-footer">
          {fault && <p role="alert" className="payment-fault">{t(fault)}</p>}
          {status && !fault && <p role="status">{t(status)}</p>}
          {!fault && !dirty && checked && <p role="status">{t("paymentBuilder.checked", { count: view.eligibleCount ?? 0 })}</p>}
          {dirty && !settings && !fault && <p className="payment-fault" role="status">{t("paymentBuilder.amountInvalid")}</p>}
          {confirming?.action === "publish" && confirmButtons(confirming)}
          <div className="payment-actions">
            {stale && <button type="button" className="key-button key-secondary" onClick={refresh}>{t("paymentBuilder.reload")}</button>}
            {retry && <button type="button" className="key-button key-primary" disabled={busy} onClick={() => void run(retry)}>{t("common.retry")}</button>}
            {!confirming && !retry && !stale && (dirty ? <>
              <button type="button" className="key-button key-secondary" disabled={busy} onClick={() => {
                setFields(toFields(view.draft)); setDirty(false); setFault(undefined);
              }}>{t("common.cancel")}</button>
              <button type="submit" className="key-button key-primary" disabled={busy || !settings}>{t("paymentBuilder.save")}</button>
            </> : <>
              <button type="button" className={`key-button ${checked ? "key-secondary" : "key-primary"}`} disabled={busy}
                onClick={() => submit({ action: "check" })}>{t("paymentBuilder.check")}</button>
              {checked && <button type="button" id="publish-payment-draft" className="key-button key-primary" disabled={busy} onClick={e => {
                returnFocus.current = e.currentTarget.id;
                setFault(undefined); setStatus(undefined); setConfirming({ action: "publish" });
              }}>{t("paymentBuilder.publish")}</button>}
            </>)}
          </div>
        </div>
      </form>
    </section>
  </div>;
}
