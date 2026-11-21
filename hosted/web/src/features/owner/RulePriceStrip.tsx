import { components, type Prices } from "../../api/catalog";
import { useLocale } from "../../i18n/LocaleProvider";
import { modelRate } from "../models/retailRate";

export const priceLabels = { input: "modelCatalog.input", cached: "modelCatalog.cacheRead", output: "modelCatalog.output" } as const;

export function PriceStrip({ prices }: { prices: Prices }) {
  const { t } = useLocale();
  return <dl className="catalog-prices">{components.map(k => <div key={k}>
    <dt>{t(priceLabels[k])}</dt><dd title={t("modelRules.units")}>${modelRate(prices[k])}<span className="sr-only"> {t("modelRules.units")}</span></dd>
  </div>)}</dl>;
}
