import { useState } from "react";
import { methodKey, type CheckoutDraft, type PaymentBuilder } from "../../api/paymentBuilder";
import { ArrowDown, ArrowUp, ChevronDown, X } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";

export type DraftFields = Pick<CheckoutDraft, "connections" | "methods" | "custom_amount"> & {
  minimum: string; quick: string; limit: string;
};

export function toFields(d: CheckoutDraft): DraftFields {
  return { connections: d.connections, methods: d.methods, custom_amount: d.custom_amount,
    minimum: String(d.minimum_credit_minor / 100), quick: d.quick_amounts.map(n => n / 100).join(" "), limit: String(d.open_limit) };
}

export function CheckoutDraftFields({ adapters, fields, noAmounts, changed }: {
  adapters: PaymentBuilder["adapters"]; fields: DraftFields; noAmounts: readonly string[]; changed: (value: DraftFields) => void;
}) {
  const { t, locale } = useLocale();
  const [connectionId, setConnection] = useState("");
  const [methodId, setMethod] = useState("");
  const usd = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" });
  const unconnected = adapters.filter(a => !fields.connections.includes(a.id));
  const connections = fields.connections.flatMap(id => adapters.filter(a => a.id === id));
  const methods = connections.flatMap(a => a.methods.map(m => ({ ...m, key: methodKey(a.id, m.code), binding: a.id, adapter: a.label })));
  const unselected = methods.filter(m => !fields.methods.some(s => methodKey(s.binding_id, s.code) === m.key));
  const addable = unselected.find(m => m.key === methodId);
  const selectMethod = (m: typeof methods[number]) => {
    if (fields.methods.length >= 16) return;
    changed({ ...fields, methods: [...fields.methods, { binding_id: m.binding, code: m.code, fee_payer: m.merchant ? "merchant" : "customer" }] });
    setMethod("");
  };
  function move(index: number, delta: number) {
    const methods = [...fields.methods];
    const item = methods[index], neighbor = methods[index + delta];
    if (!item || !neighbor) return;
    methods[index + delta] = item; methods[index] = neighbor;
    changed({ ...fields, methods });
  }

  return <>
    <div className="payment-group">
      <h3>{t("paymentBuilder.connections")}</h3>
      {adapters.length === 0 && <p className="payment-caption">{t("paymentBuilder.noAdapters")}</p>}
      {unconnected.length > 0 && fields.connections.length < 8 && <div className="payment-add">
        <div className="payment-field">
          <label htmlFor="checkout-adapter">{t("paymentBuilder.adapter")}</label>
          <span className="catalog-select">
            <select id="checkout-adapter" value={unconnected.some(a => a.id === connectionId) ? connectionId : ""} onChange={e => setConnection(e.target.value)}>
              <option value="">{t("paymentBuilder.selectAdapter")}</option>
              {unconnected.map(a => <option key={a.id} value={a.id}>{a.label}</option>)}
            </select><ChevronDown aria-hidden="true" />
          </span>
        </div>
        <button type="button" className="key-button key-secondary" disabled={!unconnected.some(a => a.id === connectionId)}
          onClick={() => { changed({ ...fields, connections: [...fields.connections, connectionId] }); setConnection(""); }}>
          {t("paymentBuilder.addConnection")}
        </button>
      </div>}
      {connections.length > 0 && <ul className="payment-list" role="list">
        {connections.map(a => <li key={a.id}><strong>{a.label}</strong><button type="button" className="text-button"
          aria-label={t("paymentBuilder.removeItem", { name: a.label })}
          onClick={() => changed({ ...fields, connections: fields.connections.filter(id => id !== a.id),
            methods: fields.methods.filter(m => m.binding_id !== a.id) })}>{t("paymentBuilder.remove")}</button></li>)}
      </ul>}
    </div>
    {connections.length > 0 && <div className="payment-group">
      <h3>{t("paymentBuilder.order")}</h3>
      {fields.methods.length > 0 && <ol className="payment-list" role="list" aria-label={t("paymentBuilder.order")}>
        {fields.methods.map((s, i) => {
          const m = methods.find(item => item.key === methodKey(s.binding_id, s.code));
          if (!m) return null;
          const up = t("paymentBuilder.moveUp", { name: m.label }), down = t("paymentBuilder.moveDown", { name: m.label });
          const remove = t("paymentBuilder.removeItem", { name: m.label });
          return <li key={m.key}>
            <div className="payment-name"><strong>{m.label}</strong>
              <small>{connections.length > 1 ? `${m.adapter} · ` : ""}{t("paymentBuilder.fee", { amount: usd.format(m.fee / 100) })}</small>
              {noAmounts.includes(m.key) && <small className="payment-ineligible">{t("paymentBuilder.noAmounts")}</small>}</div>
            <div className="payment-row-controls">
              {m.merchant && m.customer ? <span className="catalog-select">
                <select aria-label={t("paymentBuilder.whoPays", { name: m.label })} value={s.fee_payer}
                  onChange={e => changed({ ...fields, methods: fields.methods.map((item, index) => index === i
                    ? { ...item, fee_payer: e.target.value === "customer" ? "customer" : "merchant" } : item) })}>
                  <option value="merchant">{t("paymentBuilder.merchant")}</option>
                  <option value="customer">{t("paymentBuilder.customer")}</option>
                </select><ChevronDown aria-hidden="true" />
              </span> : <span className="payment-caption">{t(s.fee_payer === "customer" ? "paymentBuilder.customer" : "paymentBuilder.merchant")}</span>}
              <span className="payment-order-controls">
                {fields.methods.length > 1 && <>
                  <button type="button" className="text-button" aria-label={up} title={up} disabled={i === 0} onClick={() => move(i, -1)}><ArrowUp /></button>
                  <button type="button" className="text-button" aria-label={down} title={down} disabled={i + 1 === fields.methods.length} onClick={() => move(i, 1)}><ArrowDown /></button>
                </>}
                <button type="button" className="text-button" aria-label={remove} title={remove}
                  onClick={() => changed({ ...fields, methods: fields.methods.filter((_, index) => index !== i) })}><X /></button>
              </span>
            </div>
          </li>;
        })}
      </ol>}
      {unselected.length > 0 && fields.methods.length < 16 && <div className="payment-add">
        <div className="payment-field">
          <label htmlFor="checkout-method">{t("billing.paymentMethod")}</label>
          <span className="catalog-select">
            <select id="checkout-method" value={addable ? methodId : ""} onChange={e => setMethod(e.target.value)}>
              <option value="">{t("paymentBuilder.selectMethod")}</option>
              {unselected.map(m => <option key={m.key} value={m.key}>{connections.length > 1 ? `${m.adapter} · ` : ""}{m.label}</option>)}
            </select><ChevronDown aria-hidden="true" />
          </span>
        </div>
        <button type="button" className="key-button key-secondary" disabled={!addable}
          onClick={() => { if (addable) selectMethod(addable); }}>{t("paymentBuilder.addMethod")}</button>
      </div>}
    </div>}
    <div className="payment-group">
      <h3>{t("paymentBuilder.amounts")}</h3>
      <div className="payment-fields">
        <label className="payment-field"><span>{t("paymentBuilder.minimum")}</span>
          <input inputMode="decimal" maxLength={8} value={fields.minimum} onChange={e => changed({ ...fields, minimum: e.target.value })} /></label>
        <label className="payment-field"><span>{t("paymentBuilder.quick")}</span>
          <input id="owner-quick-amounts" inputMode="decimal" maxLength={72} aria-describedby="owner-quick-hint"
            value={fields.quick} onChange={e => changed({ ...fields, quick: e.target.value })} /></label>
        <label className="payment-field"><span>{t("paymentBuilder.limit")}</span>
          <input type="number" min={1} max={10} step={1} value={fields.limit} onChange={e => changed({ ...fields, limit: e.target.value })} /></label>
        <small className="payment-caption payment-quick-hint" id="owner-quick-hint">{t("paymentBuilder.quickHint")}</small>
      </div>
      <label className="payment-checkbox"><input type="checkbox" checked={fields.custom_amount}
        onChange={e => changed({ ...fields, custom_amount: e.target.checked })} />{t("paymentBuilder.custom")}</label>
    </div>
  </>;
}
