import { useLocale } from "../../i18n/LocaleProvider";
import { formatModelPrice, pricePresentation } from "./model";
import type { ModelPricing, ModelRateComponent } from "./model";

export function ModelPrice({ model, component }: { model: ModelPricing; component: ModelRateComponent }) {
  const { locale, t } = useLocale();
  const { price, official, discount } = pricePresentation(model, component);
  const savings = discount === null ? null : new Intl.NumberFormat(locale, { maximumFractionDigits: 20 }).format(discount);
  return <div className="catalog-price" data-component={component} data-missing={price === null || undefined}>
    <div className="catalog-price-main">
      <strong aria-label={price === null ? t("modelCatalog.unpublished") : t("modelCatalog.ourPrice", { price: formatModelPrice(price, locale) })}>{formatModelPrice(price, locale)}</strong>
      {savings !== null && <span className="catalog-saving"><span aria-hidden="true">−{savings}%</span><span className="sr-only">{t("modelCatalog.saving", { percent: savings })}</span></span>}
    </div>
    {official !== null && <span className="catalog-official"><span>{t("modelCatalog.official")}</span><s>{formatModelPrice(official, locale)}</s></span>}
  </div>;
}

/** Cache TTL prices are separate core components, never a generic write price. */
export function ModelCacheWritePrice({ model }: { model: ModelPricing }) {
  const { t } = useLocale();
  const components = (["cacheWrite", "cacheWrite5m", "cacheWrite1h"] as const).filter(component => model.rates[component] != null);
  if (!components.length || components.length === 1 && components[0] === "cacheWrite") return <ModelPrice model={model} component="cacheWrite" />;
  return <div className="catalog-cache-rates">{components.map(component => <div key={component}>
    <span className="catalog-cache-label">{t(`modelCatalog.${component}`)}</span>
    <ModelPrice model={model} component={component} />
  </div>)}</div>;
}
