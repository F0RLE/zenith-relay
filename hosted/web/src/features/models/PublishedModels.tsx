import { useEffect, useState } from "react";
import { components, readListings, type Listing, type Prices } from "../../api/catalog";
import { ControlError, observeControl } from "../../api/control";
import { useLocale } from "../../i18n/LocaleProvider";
import { modelRate } from "./retailRate";

export const priceLabels = { input: "modelCatalog.input", cached: "modelCatalog.cacheRead", output: "modelCatalog.output" } as const;
export function PriceStrip({ prices }: { prices: Prices }) {
  const { t } = useLocale();
  return <dl className="catalog-prices">{components.map(k => <div key={k}>
    <dt>{t(priceLabels[k])}</dt><dd title={t("modelRules.units")}>${modelRate(prices[k])}<span className="sr-only"> {t("modelRules.units")}</span></dd>
  </div>)}</dl>;
}

export function PublishedModels() {
  const { t } = useLocale();
  const [rows, setRows] = useState<Listing[]>();
  const [failed, setFailed] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => observeControl(readListings,
    v => { setRows(v); setFailed(false); }, err => {
      if (err instanceof ControlError && err.status === 401) location.replace("/login");
      else { setRows(undefined); setFailed(true); }
    }), [reload]);
  if (failed) return <div><p role="alert">{t("unavailable.server")}</p><button className="key-button key-secondary"
    onClick={() => { setFailed(false); setReload(n => n + 1); }}>{t("common.retry")}</button></div>;
  if (!rows) return <p role="status">{t("unavailable.connecting")}</p>;
  if (!rows.length) return <p>{t("modelStatus.noModels")}</p>;
  return <div className="entry-catalog">
    <ul className="catalog-list">{rows.map(row => <li key={row.model} className="catalog-listing">
      <div className="catalog-identity"><strong>{row.model}</strong><small>Responses</small></div><PriceStrip prices={row.rates} />
    </li>)}</ul></div>;
}
