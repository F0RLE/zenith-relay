import { useRef, useState } from "react";
import { parseCreditInput, readTopUp, type TopUp } from "../../api/checkout";
import { ControlError } from "../../api/control";
import { refundIsOpen, refundLabels, requestRefund } from "../../api/refunds";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { formatMoney } from "../usage/model";
import "./refunds.css";

export function OwnRefund({ payment, csrf, disabled, onUpdate }: {
  payment: TopUp; csrf: string; disabled: boolean; onUpdate: (p: TopUp) => void;
}) {
  const { t, locale } = useLocale();
  const [editing, setEditing] = useState(false);
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState<MessageKey>();
  const attempt = useRef<{ id: string; amount: bigint } | null>(null);
  const running = useRef(false);
  const trigger = useRef<HTMLButtonElement>(null), field = useRef<HTMLInputElement>(null);
  const status = useRef<HTMLSpanElement>(null);
  const current = payment.refund;
  const canRequest = payment.refundable > 0n && (!current || !refundIsOpen(current.state));
  const amount = parseCreditInput(input);
  const valid = amount !== null && amount > 0n && amount <= payment.refundable;
  const money = (n: bigint) => `${formatMoney(n, 2, locale)} USD`;

  async function submit() {
    if (running.current || disabled || (!attempt.current && (!valid || amount === null))) return;
    const op = attempt.current ?? { id: crypto.randomUUID(), amount: amount! };
    attempt.current = op; running.current = true; setPending(true); setProblem(undefined);
    try {
      await requestRefund(csrf, payment.id, op.id, op.amount);
      const updated = await readTopUp(payment.id);
      onUpdate(updated);
      attempt.current = null; setInput(""); setEditing(false);
      requestAnimationFrame(() => status.current?.focus());
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) { location.replace("/login"); return; }
      if (e instanceof ControlError && ["refund_remainder", "refund_conflict"].includes(e.code ?? "")) {
        attempt.current = null;
        setProblem(e.code === "refund_remainder" ? "refund.remainderChanged" : "refund.changed");
        const updated = await readTopUp(payment.id).catch(() => null);
        if (updated) onUpdate(updated);
      } else {
        setProblem("refund.unconfirmed");
      }
    } finally { running.current = false; setPending(false); }
  }

  if (!current && !canRequest) return null;
  return <div className="payment-refund" aria-busy={pending}>
    <div className="refund-line">
      {current && <span ref={status} tabIndex={-1} className="refund-state" data-state={current.state}>
        {t(refundLabels[current.state])} · {money(current.amount)}</span>}
      {current?.receipt && current.state === "succeeded" && <a className="entry-account-link checkout-receipt"
        href={"/app/billing?id=" + current.receipt}>{t("refund.receipt")}</a>}
      {canRequest && !editing && <button type="button" className="button refund-trigger" ref={trigger} disabled={disabled}
        onClick={() => { setEditing(true); requestAnimationFrame(() => field.current?.focus()); }}>{t("refund.request")}</button>}
    </div>
    {editing && canRequest && <form className="owner-setup-form refund-form" onSubmit={e => { e.preventDefault(); void submit(); }}>
      <label className="checkout-field">{t("refund.amount")}
        <input ref={field} inputMode="decimal" maxLength={8} autoComplete="off" value={input}
          disabled={disabled || pending || !!attempt.current} aria-invalid={!!input && !valid}
          onChange={e => { setInput(e.target.value); setProblem(undefined); }} />
      </label>
      <small>{t("refund.limit", { amount: money(payment.refundable) })}</small>
      <div className="refund-actions">
        <button type="button" className="key-button key-secondary" disabled={disabled || pending} onClick={() => {
          setEditing(false); setProblem(undefined);
          requestAnimationFrame(() => trigger.current?.focus());
        }}>{t("common.cancel")}</button>
        <button type="submit" className="key-button key-primary" disabled={disabled || pending || (!valid && !attempt.current)}>
          {t(problem === "refund.unconfirmed" ? "common.retry" : "refund.sendRequest")}</button>
      </div>
    </form>}
    {problem && <span className="refund-problem" role="alert">{t(problem)}</span>}
  </div>;
}
