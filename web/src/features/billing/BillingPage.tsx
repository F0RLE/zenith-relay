import { useEffect, useId, useRef, useState } from "react";
import { Plus } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { OptionMenu } from "../../components/OptionMenu";
import { useFormatters } from "../../i18n/format";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { BillingHistory } from "./BillingHistory";
import { parseTopUpAmount, quickTopUpAmounts, topUpLimits } from "./model";
import type { PaymentMethod, TopUpRequest } from "./model";

export interface PaymentOperation { id: string; at: string; kind: string; amount: bigint; status: string; method?: string }
export function BillingPage({ balance, operations, paymentMethods, asOf, onTopUp, active = true }: { balance: bigint; operations: PaymentOperation[]; paymentMethods: readonly PaymentMethod[]; asOf: string; onTopUp: (request: TopUpRequest) => void; active?: boolean }) {
  const { t, localize } = useLocale();
  const { formatMoney } = useFormatters();
  const [dialog, setDialog] = useState<PaymentOperation | null>(null);
  const [amount, setAmount] = useState("25");
  const [methodId, setMethodId] = useState(paymentMethods[0]?.id ?? "");
  const [error, setError] = useState<MessageKey | "">("");
  const [receipt, setReceipt] = useState<{ amount: bigint; method: string } | null>(null);
  const amountInput = useRef<HTMLInputElement>(null);
  const formId = useId();
  const parsedAmount = parseTopUpAmount(amount);
  const selectedMethod = paymentMethods.find(method => method.id === methodId);
  useEffect(() => { if (!active) setDialog(null); }, [active]);
  function updateAmount(value: string) { setAmount(value); setError(""); setReceipt(null); }
  function submitTopUp() {
    if (parsedAmount === null) {
      setError("billing.invalidAmount");
      amountInput.current?.focus();
      return;
    }
    if (!selectedMethod) { setError("billing.chooseAPaymentMethod"); return; }
    onTopUp({ microUsd: parsedAmount, methodId: selectedMethod.id });
    setReceipt({ amount: parsedAmount, method: selectedMethod.label });
    setAmount("");
    setError("");
  }
  return <>
    <header className="page-heading"><h1>{t("nav.balance")}</h1></header>
    <div className="billing-layout">
    <section className="balance-panel" aria-label={t("nav.balance")}><div>
      <div><p>{t("billing.availableBalance")}</p><strong>{formatMoney(balance, 2)} USD</strong></div>
      <form className="balance-topup-form" aria-label={t("billing.topUpBalance")} aria-describedby={!paymentMethods.length ? `${formId}-unavailable` : undefined} noValidate onSubmit={event => { event.preventDefault(); submitTopUp(); }}>
        <div className="topup-amount-field"><label htmlFor={`${formId}-amount`}>{t("billing.amountUsd")}</label><input ref={amountInput} id={`${formId}-amount`} type="number" inputMode="numeric" min={topUpLimits.min} max={topUpLimits.max} step={topUpLimits.step} required placeholder={t("billing.amountPlaceholder")} value={amount} aria-invalid={!!error && parsedAmount === null} aria-describedby={error ? `${formId}-error` : undefined} onChange={event => updateAmount(event.target.value)} />
          <div className="topup-presets" role="group" aria-label={t("billing.quickAmount")}>{quickTopUpAmounts.map(preset => <button key={preset} type="button" aria-label={`${preset} USD`} aria-pressed={parsedAmount === BigInt(preset) * 1_000_000n} onClick={() => updateAmount(String(preset))}>{preset}</button>)}</div>
        </div>
        <div className="topup-method-field"><span>{t("billing.paymentMethod")}</span><OptionMenu label={t("billing.paymentMethod")} value={selectedMethod?.id ?? ""} options={[{ value: "", label: t(paymentMethods.length ? "billing.chooseAMethod" : "billing.notAvailableYet") }, ...paymentMethods.map(method => ({ value: method.id, label: localize(method.label) }))]} disabled={!paymentMethods.length} onChange={value => { setMethodId(value); setError(""); setReceipt(null); }} /></div>
        {error && <p className="topup-error" id={`${formId}-error`} role="alert">{t(error)}</p>}
        <button type="submit" className="button primary-button" disabled={!paymentMethods.length || !amount.trim()}><Plus aria-hidden="true" />{t("billing.topUp")}</button>
        {!paymentMethods.length && <p className="topup-error" id={`${formId}-unavailable`}>{t("billing.paymentMethodsAreNotAvailableYet")}</p>}
        <p className="topup-receipt" role="status">{receipt && t("billing.receipt", { amount: formatMoney(receipt.amount, 2), method: localize(receipt.method) })}</p>
      </form>
    </div></section>
    <BillingHistory operations={operations} asOf={asOf} onSelect={setDialog} />
    </div>
    {dialog && <Dialog title={t("billing.paymentOperation")} onClose={() => setDialog(null)}>
      <dl className="detail-list"><div><dt>{t("billing.operationId")}</dt><dd><code>{dialog.id}</code></dd></div><div><dt>{t("common.type")}</dt><dd>{localize(dialog.kind)}</dd></div><div><dt>{t("common.amount")}</dt><dd>{formatMoney(dialog.amount, 2)} USD</dd></div>{dialog.method && <div><dt>{t("billing.paymentMethod")}</dt><dd>{localize(dialog.method)}</dd></div>}<div><dt>{t("common.status")}</dt><dd>{localize(dialog.status)}</dd></div></dl>
    </Dialog>}
  </>;
}
