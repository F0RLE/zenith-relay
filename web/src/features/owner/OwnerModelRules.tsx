import { useEffect, useId, useState } from "react";
import { OptionMenu } from "../../components/OptionMenu";
import { ChevronDown, GripVertical } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { modelContextRanges, selectedModelVariant } from "../models/model";
import type { CustomerModel, ModelRateComponent, ModelRates } from "../models/model";
import { applyOwnerModelRule, modelRateInput, parseModelRate, poolRuleModels, setOwnerModelRate } from "./modelRules";
import type { OwnerModelRules } from "./modelRules";
import type { OwnerConnection } from "./model";
import { catalogGroups, moveCatalogItem } from "../models/catalogOrder";
import type { CatalogMove } from "../models/catalogOrder";
import { useCatalogReorder } from "./useCatalogReorder";
import "./owner-model-rules.css";

const writeComponents: ModelRateComponent[] = ["cacheWrite", "cacheWrite5m", "cacheWrite1h"];
const tierLabels: Record<string, MessageKey> = {
  standard: "modelCatalog.standardTier", flex: "modelCatalog.flexTier", priority: "modelCatalog.priorityTier", ultrafast: "modelCatalog.ultrafastTier",
};

function RateInput({ label, name, value, detail = false, onChange }: { label: string; name: string; value: bigint | null | undefined; detail?: boolean; onChange: (value: bigint | null) => void }) {
  const { t } = useLocale();
  const id = useId();
  const [draft, setDraft] = useState(() => modelRateInput(value));
  const [invalid, setInvalid] = useState(false);
  useEffect(() => { setDraft(modelRateInput(value)); setInvalid(false); }, [value]);
  function commit(text: string) {
    const parsed = parseModelRate(text);
    setInvalid(parsed === undefined);
    if (parsed === undefined) return;
    setDraft(modelRateInput(parsed));
    if (parsed !== (value ?? null)) onChange(parsed);
  }
  return <label className="owner-model-rate" data-detail={detail}><span>{label}</span>
    <input type="text" inputMode="decimal" autoComplete="off" spellCheck={false} aria-label={name} aria-invalid={invalid || undefined} aria-describedby={invalid ? id : undefined} placeholder="—" value={draft}
      onChange={event => { setDraft(event.target.value); setInvalid(false); }} onBlur={event => commit(event.currentTarget.value)}
      onKeyDown={event => {
        if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur(); }
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); event.currentTarget.value = modelRateInput(value); setDraft(event.currentTarget.value); setInvalid(false); event.currentTarget.blur(); }
      }} />
    {invalid && <small id={id} role="alert">{t("modelRules.invalidPrice")}</small>}
  </label>;
}

export function OwnerModelRulesView({ models, connections, rules, onChange, displayOrder = [], onOrderChange, active = true }: {
  models: CustomerModel[]; connections: OwnerConnection[]; rules: OwnerModelRules; onChange: (rules: OwnerModelRules) => void;
  displayOrder?: readonly string[]; onOrderChange?: ((order: string[]) => void) | undefined; active?: boolean;
}) {
  const { t, locale } = useLocale();
  const [priceVariants, setPriceVariants] = useState<Record<string, string>>({});
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const modelRowsId = useId();
  const inventory = poolRuleModels(models, connections);
  const groups = catalogGroups(inventory, displayOrder);
  const dragHelp = useId();
  const [announcement, setAnnouncement] = useState("");
  const groupLabel = (group: (typeof groups)[number]) => group.provider === "other" ? t("modelCatalog.otherCompany") : group.label;
  function move(item: CatalogMove) {
    // The UI exposes pooled models, but the canonical order retains every catalog entry.
    const next = moveCatalogItem(models, displayOrder, item);
    if (!next || !onOrderChange) return;
    onOrderChange(next);
    const updated = catalogGroups(inventory, next);
    const group = updated.find(group => group.id === item.source || group.models.some(model => model.id === item.source));
    if (!group) return;
    const peers = item.kind === "group" ? updated : group.models;
    const position = peers.findIndex(peer => peer.id === item.source);
    const name = item.kind === "group" ? groupLabel(group) : group.models.find(model => model.id === item.source)!.name;
    setAnnouncement(t("modelRules.position", { name, position: position + 1, total: peers.length }));
  }
  const reorder = useCatalogReorder(groups, move, active);
  function handle(kind: CatalogMove["kind"], id: string, name: string, count: number) {
    return <button type="button" className="owner-model-drag-handle" data-kind={kind} disabled={!onOrderChange || count < 2}
      aria-label={t(kind === "group" ? "modelRules.moveCompany" : "modelRules.moveModel", { name })} aria-describedby={dragHelp} aria-keyshortcuts="ArrowUp ArrowDown Home End"
      data-relay-tooltip={t("modelRules.dragHint")} onPointerDown={event => reorder.start(event, { kind, id })} onKeyDown={event => reorder.key(event, { kind, id })}>
      <GripVertical aria-hidden="true" />
    </button>;
  }
  function offerLabel(model: CustomerModel, variant: NonNullable<CustomerModel["pricingVariants"]>[number]) {
    const range = modelContextRanges(model, variant.tier).find(item => item.band === variant.contextBand);
    const context = [range?.min != null ? `> ${range.min.toLocaleString(locale)}` : "", range?.max != null ? `≤ ${range.max.toLocaleString(locale)}` : ""].filter(Boolean).join(" – ");
    const tier = variant.tier === "standard" ? "" : tierLabels[variant.tier] ? t(tierLabels[variant.tier]!) : variant.tier;
    return [tier, context].filter(Boolean).join(" · ");
  }
  const rateLabel = (component: ModelRateComponent) => component === "cacheWrite5m" || component === "cacheWrite1h"
    ? `${t("modelCatalog.cacheWrite")} · ${t(`modelCatalog.${component}`)}` : t(`modelCatalog.${component}` as MessageKey);
  function field(model: CustomerModel, rates: ModelRates, component: ModelRateComponent, variantId?: string) {
    return <RateInput key={`${variantId ?? "base"}:${component}`} label={rateLabel(component)} detail={component === "cacheWrite5m" || component === "cacheWrite1h"}
      name={t("modelRules.priceFor", { component: rateLabel(component), name: model.name })} value={rates[component]}
      onChange={value => onChange(setOwnerModelRate(rules, model, variantId, component, value))} />;
  }
  return <section ref={reorder.root} className="owner-model-rules" aria-label={t("modelRules.title")}>
    <span id={dragHelp} className="sr-only">{t("modelRules.dragHelp")}</span>
    <span className="sr-only" aria-live="polite" aria-atomic="true">{announcement}</span>
    <table><caption><span className="sr-only">{t("modelRules.title")} · </span>{t("modelRules.units")}</caption>
      <colgroup><col className="owner-model-identity-column" /><col className="owner-model-offer-column" /><col span={4} /><col className="owner-model-enabled-column" /></colgroup>
      <thead><tr><th scope="col">{t("modelRules.model")}</th><th scope="col">{t("modelRules.offer")}</th>
        <th scope="col">{t("modelCatalog.input")}</th><th scope="col">{t("modelCatalog.output")}</th>
        <th scope="col">{t("modelCatalog.cacheRead")}</th><th scope="col">{t("modelCatalog.cacheWrite")}</th>
        <th scope="col">{t("modelRules.enabled")}</th>
      </tr></thead>
      {groups.map(group => <tbody key={group.id} data-group-id={group.id} data-collapsed={collapsedGroups.has(group.id) || undefined} {...reorder.state("group", group.id)}>
        <tr className="owner-model-group"><th scope="rowgroup" colSpan={7}><div className="owner-model-group-heading">
          <button type="button" className="owner-model-group-toggle" aria-expanded={!collapsedGroups.has(group.id)}
            aria-label={t(collapsedGroups.has(group.id) ? "modelRules.expandCompany" : "modelRules.collapseCompany", { name: groupLabel(group) })}
            aria-controls={group.models.map(model => `${modelRowsId}-${encodeURIComponent(model.id)}`).join(" ")}
            onClick={() => setCollapsedGroups(current => {
              const next = new Set(current);
              if (next.has(group.id)) next.delete(group.id); else next.add(group.id);
              return next;
            })}>
            <ChevronDown aria-hidden="true" />
          </button>
          {handle("group", group.id, groupLabel(group), groups.length)}
          <strong>{groupLabel(group)}</strong><small>{group.models.length.toLocaleString(locale)}</small>
        </div></th></tr>
        {group.models.map(original => {
          const model = applyOwnerModelRule(original, rules[original.id]);
          const enabled = rules[model.id]?.enabled !== false;
          const priceVariant = selectedModelVariant(model, priceVariants[model.id]);
          const priceLabel = priceVariant ? offerLabel(model, priceVariant) : "";
          const priceOptions = (model.pricingVariants ?? []).map(variant => {
            const label = offerLabel(model, variant);
            return { value: variant.id, label: label || "—", shortLabel: label };
          });
          const rates = priceVariant?.rates ?? model.rates;
          const writes = writeComponents.filter(component => component === "cacheWrite" && rates.cacheWrite5m === undefined && rates.cacheWrite1h === undefined || Object.hasOwn(rates, component));
          return <tr key={model.id} id={`${modelRowsId}-${encodeURIComponent(model.id)}`} hidden={collapsedGroups.has(group.id)} className="owner-model-row" data-model-id={model.id} data-enabled={enabled} {...reorder.state("model", model.id)}>
            <th scope="row"><div className="owner-model-name-cell">{handle("model", model.id, model.name, group.models.length)}<div><div className="owner-model-identity"><strong>{model.name}</strong></div><code>{model.id}</code></div></div>
            </th>
            <td className="owner-model-offer" data-available={priceOptions.length > 1 || Boolean(priceLabel)}><span className="owner-model-offer-label">{t("modelRules.offer")}</span>
              {priceOptions.length > 1 && priceVariant
                ? <OptionMenu label={t("modelCatalog.tierFor", { name: model.name })} value={priceVariant.id} onChange={value => setPriceVariants(current => ({ ...current, [model.id]: value }))} options={priceOptions} />
                : <span className="owner-model-offer-value">{priceLabel}</span>}
            </td>
            {(["input", "output", "cacheRead"] as const).map(component => <td key={component}>{field(original, rates, component, priceVariant?.id)}</td>)}
            <td><div className="owner-model-cache-fields">{writes.map(component => field(original, rates, component, priceVariant?.id))}</div></td>
            <td className="owner-model-availability"><span>{t("modelRules.enabled")}</span>
              <label className="owner-model-toggle" data-relay-tooltip={t(enabled ? "modelRules.disable" : "modelRules.enable", { name: model.name })}>
                <input type="checkbox" role="switch" aria-label={t("modelRules.enabledFor", { name: model.name })} checked={enabled} onChange={event => onChange({ ...rules, [model.id]: { ...rules[model.id], enabled: event.target.checked } })} />
              </label>
            </td>
          </tr>;
        })}</tbody>)}
    </table>
    {!inventory.length && <p className="owner-model-rules-empty" role="status">{t("modelRules.empty")}</p>}
  </section>;
}
