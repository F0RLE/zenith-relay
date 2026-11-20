import { useState } from "react";
import { components, textRate, type MarkupRule, type Prices, type Rule, type RuleAction, type RuleFields, type SaleMode } from "../../api/catalog";
import { ChevronDown } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { PriceStrip, priceLabels } from "../models/PublishedModels";
import { modelRate } from "../models/retailRate";

export type ApplyRule = (row: Rule, action: RuleAction, fields?: RuleFields) => Promise<void>;
const blank = { input: "", cached: "", output: "" };
const same = (a: Prices | null, b: Prices | null) => !!a && !!b && components.every(k => a[k] === b[k]);
const showRate = (s: string | null) => modelRate(s) ?? "";
const inputRate = (s: string) => textRate(s.replace(",", "."));
const displayPrices = (p: Prices | null): Prices => p
  ? { input: showRate(p.input), cached: showRate(p.cached), output: showRate(p.output) } : blank;
const modes = { fixed: "catalog.fixedPrices", api_default: "catalog.useApiDefault", model_markup: "catalog.modelMarkup" } as const;

function editPrices(values: Prices, saved: Prices | null) {
  const [input, cached, output] = components.map(k => inputRate(values[k]));
  const prices = input && cached && output ? { input, cached, output } : undefined;
  return {
    prices,
    changed: prices ? !same(prices, saved) : saved !== null || components.some(k => values[k] !== ""),
    invalid: components.some(k => values[k] !== "" && inputRate(values[k]) === null),
  };
}

function RateFields({ values, onChange }: { values: Prices; onChange: (values: Prices) => void }) {
  const { t } = useLocale();
  return <div className="catalog-fields">{components.map(k => <label key={k}><span>{t(priceLabels[k])}</span>
    <input value={values[k]} inputMode="decimal" autoComplete="off" spellCheck={false} maxLength={25}
      aria-invalid={values[k] !== "" && inputRate(values[k]) === null} placeholder="—"
      onChange={e => onChange({ ...values, [k]: e.currentTarget.value })} /></label>)}</div>;
}

type ModeEdit = { mode: SaleMode; percent: string };
function PriceForm({ source, offer, mode, onMode, busy, apply }: {
  source: Rule; offer: Rule; mode: ModeEdit; onMode: (m: ModeEdit) => void; busy: boolean; apply: ApplyRule;
}) {
  const { t } = useLocale();
  const rule = offer.sale_rule;
  const [buy, setBuy] = useState(() => displayPrices(source.purchase));
  const [fixed, setFixed] = useState(() => displayPrices(offer.draft));
  const [base, setBase] = useState(() => showRate(source.source_markup.percent));
  const [confirm, setConfirm] = useState(false);
  const purchase = editPrices(buy, source.purchase), sale = editPrices(fixed, offer.draft);
  const defaultMarkup = base === "" ? null : inputRate(base) ?? undefined;
  const percent = inputRate(mode.percent);
  const fromMarkup = mode.mode !== "fixed";
  const markupInvalid = mode.mode === "model_markup" && mode.percent !== "" && percent === null;
  const defaultInvalid = defaultMarkup === undefined;
  const defaultChanged = defaultMarkup !== source.source_markup.percent;
  const saleChanged = !fromMarkup && (sale.changed || rule.mode !== "fixed");
  const changed = purchase.changed || saleChanged || defaultChanged;
  const complete = (!purchase.changed || !!purchase.prices) && (!saleChanged || !!sale.prices);
  const needsReview = fromMarkup && (rule.mode !== mode.mode || (mode.mode === "model_markup" && rule.percent !== percent) || rule.stale);
  const needsPublication = changed || needsReview || !same(offer.draft, offer.effective);
  const reviewed = fromMarkup && mode.mode === rule.mode && (mode.mode !== "model_markup" || percent === rule.percent);
  const review: MarkupRule | undefined = mode.mode === "api_default" ? { mode: "api_default" }
    : mode.mode === "model_markup" && percent !== null ? { mode: "model_markup", percent } : undefined;
  const hint = purchase.invalid || (!fromMarkup && sale.invalid) || markupInvalid || defaultInvalid ? "catalog.invalidRate"
    : !complete ? "catalog.incompleteRates" : fromMarkup && rule.stale ? "catalog.reviewStale"
    : offer.draft && offer.ready && !offer.supply_ready ? "catalog.purchaseRequired"
    : !saleChanged && (!fromMarkup || reviewed) && rule.below_cost ? "catalog.belowCost" : undefined;

  return <>
    {offer.effective && needsPublication && <div className="catalog-effective"><span>{t("catalog.current")}</span><PriceStrip prices={offer.effective} /></div>}
    {!source.ready && <small>{t("catalog.checkRequired")}</small>}
    <form className="catalog-form owner-setup-form" onSubmit={e => {
      e.preventDefault();
      if (!complete || !changed || busy || defaultInvalid) return;
      void apply(source, "save", {
        ...(purchase.changed && purchase.prices && { purchase_rates: purchase.prices }),
        ...(saleChanged && sale.prices && { rates: sale.prices }),
        ...(defaultChanged && { source_markup: { expected_revision: source.source_markup.revision, percent: defaultMarkup } }),
      });
    }}>
      <div className="catalog-edit-prices">
        <fieldset disabled={busy}>
          <legend>{t("catalog.purchase")}</legend>
          <RateFields values={buy} onChange={values => { setConfirm(false); setBuy(values); }} />
          <label className="catalog-markup-field"><span>{t("catalog.defaultMarkup")}</span>
            <input value={base} inputMode="decimal" autoComplete="off" spellCheck={false} maxLength={25}
              aria-invalid={defaultInvalid} placeholder="—" onChange={e => { setConfirm(false); setBase(e.currentTarget.value); }} /></label>
        </fieldset>
        <fieldset disabled={busy}>
          <legend>{t("catalog.sale")}</legend>
          <div className="catalog-select">
            <select aria-label={t("catalog.pricingMode")} value={mode.mode} onChange={e => {
              setConfirm(false); onMode({ ...mode, mode: e.currentTarget.value as SaleMode });
            }}>{(Object.keys(modes) as SaleMode[]).map(k => <option key={k} value={k}>{t(modes[k])}</option>)}</select>
            <ChevronDown aria-hidden="true" />
          </div>
          {!fromMarkup ? <RateFields values={fixed} onChange={values => { setConfirm(false); setFixed(values); }} /> : <>
            {mode.mode === "model_markup" && <label className="catalog-markup-field"><span>{t("catalog.markupPercent")}</span>
              <input value={mode.percent} inputMode="decimal" autoComplete="off" spellCheck={false} maxLength={25}
                aria-invalid={markupInvalid} onChange={e => onMode({ ...mode, percent: e.currentTarget.value })} /></label>}
            {reviewed && offer.draft && <div className="catalog-effective"><span>{t("catalog.reviewed")}</span><PriceStrip prices={offer.draft} /></div>}
          </>}
        </fieldset>
      </div>
      {hint && <small className="catalog-price-hint" role="status">{t(hint)}</small>}
      <div className="catalog-actions">
        {changed && <button className="key-button key-secondary" type="submit" disabled={busy || !complete || defaultInvalid}>{t("catalog.savePrices")}</button>}
        {needsReview && <button className="key-button key-secondary" type="button" disabled={busy || changed || !review}
          onClick={() => { if (review) void apply(offer, "review", { sale_rule: review }); }}>{t("catalog.reviewPrices")}</button>}
        {needsPublication && <button className="key-button key-primary" type="button"
          disabled={busy || !offer.supply_ready || changed || needsReview || !offer.draft}
          onClick={() => void apply(offer, "publish")}>{t("catalog.publish")}</button>}
        {offer.enabled && <>
          {offer.effective && <span className="catalog-live">{t("catalog.published")}</span>}
          <button type="button" className="key-button key-secondary" disabled={busy}
            onClick={() => confirm ? void apply(offer, "suspend") : setConfirm(true)}>{t(confirm ? "catalog.confirmWithdraw" : "catalog.withdraw")}</button>
          {confirm && <button type="button" className="key-button key-secondary" disabled={busy} onClick={() => setConfirm(false)}>{t("common.cancel")}</button>}
        </>}
      </div>
    </form>
  </>;
}

export function ModelPriceEditor({ sources, busy, apply }: { sources: Rule[]; busy: boolean; apply: ApplyRule }) {
  const { t } = useLocale();
  const [selected, setSelected] = useState<string>();
  const [mode, setMode] = useState<ModeEdit & { origin: string }>();
  const live = sources.find(r => r.enabled);
  const source = sources.find(r => r.id === selected) ?? live ?? sources.find(r => r.ready) ?? sources[0]!;
  const offer = live ?? source;
  const origin = `${offer.id}:${offer.sale_rule.revision}`;
  const editorMode = mode?.origin === origin ? mode : { mode: offer.sale_rule.mode, percent: showRate(offer.sale_rule.percent) };
  return <li className="catalog-editor">
    <div className="catalog-identity"><strong>{offer.model}</strong>
      {sources.length === 1 ? <small>{source.source}</small> : <div className="catalog-select catalog-source-select">
        <select aria-label={t("connections.source")} disabled={busy} value={source.id}
          onChange={e => setSelected(e.currentTarget.value)}>{sources.map(r => <option key={r.id} value={r.id}>{r.source}</option>)}</select>
        <ChevronDown aria-hidden="true" />
      </div>}</div>
    <PriceForm key={`${source.id}:${source.revision}:${source.source_markup.revision}:${offer.revision}`}
      source={source} offer={offer} mode={editorMode} onMode={v => setMode({ ...v, origin })} busy={busy} apply={apply} />
  </li>;
}
