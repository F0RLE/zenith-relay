import { useId, useState } from "react";
import { CopyButton } from "../../components/CopyButton";
import { useLocale } from "../../i18n/LocaleProvider";
import { CatalogChoice } from "./CatalogChoice";
import { ModelCacheWritePrice, ModelPrice } from "./ModelPrice";
import { modelContextRanges, selectedModelVariant } from "./model";
import type { CustomerModel, ModelPricingVariant } from "./model";
import { catalogGroups } from "./catalogOrder";
import "./models.css";

export type { CustomerModel } from "./model";

const components = [
  ["input", "modelCatalog.input"], ["output", "modelCatalog.output"],
  ["cacheRead", "modelCatalog.cacheRead"], ["cacheWrite", "modelCatalog.cacheWrite"],
] as const;

function ModelRow({ model }: { model: CustomerModel }) {
  const { t, locale } = useLocale();
  const [selectedId, setSelectedId] = useState<string>();
  const selected = selectedModelVariant(model, selectedId);
  const variants = model.pricingVariants ?? [];
  const ranges = selected ? modelContextRanges(model, selected.tier) : [];
  const contexts = ranges.map(range => variants.find(variant => variant.tier === selected?.tier && variant.contextBand === range.band)!);
  const compact = (value: number | null | undefined) => typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(value) : "—";
  const contextText = (variant: ModelPricingVariant, short = false) => {
    const range = ranges.find(range => range.band === variant.contextBand)!;
    if (range.min == null && range.max == null) return null;
    if (range.min != null && range.max != null) return `> ${compact(range.min)} · ≤ ${compact(range.max)}`;
    if (short) return range.min == null ? `≤ ${compact(range.max)}` : `> ${compact(range.min)}`;
    return t(range.min == null ? "modelCatalog.shortContext" : "modelCatalog.longContext", { tokens: compact(range.min ?? range.max) });
  };
  return <tr className="catalog-row" data-model-id={model.id} data-variant-id={selected?.id}>
    <th scope="row"><div className="catalog-model-heading"><strong className="catalog-model-name">{model.name}</strong></div><code className="catalog-model-id">{model.id}</code>
    </th>
    <td className="catalog-protocol-cell"><span className="catalog-cell-label">{t("modelCatalog.protocol")}</span><span className="catalog-protocols">{model.protocols.length ? model.protocols.join(" · ") : "—"}</span></td>
    {components.map(([component, label]) => <td className="catalog-rate-cell" key={component}><span className="catalog-cell-label">{t(label)}</span>{component === "cacheWrite" ? <ModelCacheWritePrice model={selected ?? model} /> : <ModelPrice model={selected ?? model} component={component} />}</td>)}
    <td className="catalog-context"><span className="catalog-cell-label">{t("modelCatalog.context")}</span><div className="catalog-context-value">
      {selected && contextText(selected)
        ? <CatalogChoice label={t("modelCatalog.contextFor", { name: model.name })} description={t("modelCatalog.maxContext", { tokens: compact(model.contextTokens) })} value={selected.id} options={contexts.map(variant => ({ value: variant.id, label: contextText(variant)!, shortLabel: contextText(variant, true)! }))} onChange={setSelectedId} />
        : <span>{compact(model.contextTokens)}</span>}
    </div></td>
    <td className="catalog-actions"><CopyButton text={model.id} label={t("modelCatalog.copyId", { name: model.name })} compact /></td>
  </tr>;
}

export function ModelsPage({ models = [] }: {
  models?: readonly CustomerModel[];
}) {
  const { t } = useLocale();
  const heading = useId();
  const groups = catalogGroups(models);

  return <section className="models-page">
    <header className="page-heading"><h1 id={heading}>{t("modelCatalog.title")}</h1></header>
    <section className="table-shell catalog-frame" aria-labelledby={heading}>
      <table className="catalog-table">
        <caption className="sr-only">{t("modelCatalog.table")}</caption>
        <colgroup><col className="catalog-name-col" /><col className="catalog-protocol-col" />{components.map(([component]) => <col key={component} />)}<col className="catalog-context-col" /><col className="catalog-action-col" /></colgroup>
        <thead><tr><th scope="col">{t("common.model")}</th><th scope="col">{t("modelCatalog.protocol")}</th>{components.map(([component, label]) => <th key={component} scope="col">{t(label)}</th>)}<th scope="col">{t("modelCatalog.context")}</th><th scope="col"><span className="sr-only">{t("modelCatalog.actions")}</span></th></tr></thead>
        {groups.map(group => <tbody key={group.id} data-group-id={group.id}>
          <tr className="catalog-company"><th scope="rowgroup" colSpan={8}>{group.provider === "other" ? t("modelCatalog.otherCompany") : group.label}</th></tr>
          {group.models.map(model => <ModelRow key={model.id} model={model} />)}
        </tbody>)}
        {!models.length && <tbody><tr className="catalog-empty"><td colSpan={8}><p role="status">{t("modelCatalog.noModels")}</p></td></tr></tbody>}
      </table>
    </section>
  </section>;
}
