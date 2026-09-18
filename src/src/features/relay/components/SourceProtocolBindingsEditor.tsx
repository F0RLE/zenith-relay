import { type CSSProperties, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CacheWriteTtl, SourceAdapter, SourceProtocolBinding, SourceSummary, SourceWireApi } from "../api/types";
import { adapterBetween, effectiveSourceProtocolBindings, normalizedAdapter, normalizedBindings, sourceWireApis, upstreamWireApi } from "../sourceProtocolBindings";
import { OptionMenu, Tabs } from "./Ui";
import { updateCacheWriteTtl, updateModelRoute, updateNativeProtocol } from "./sourceProtocolBindingsEditorModel";
import { protocolPresentation, simpleRouteCards } from "./sourceProtocolPresentation";

export type SourceProtocolBindingsEditorProps = {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
  showSimplePicker?: boolean;
  autoAssignModels?: boolean;
  exclusiveSimplePicker?: boolean;
  routeGroup?: "all" | "native" | "adapters";
};

export function SourceProtocolBindingsSummary({ source }: {
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models" | "protocolConfig" | "resolvedProtocolBindings">;
}) {
  const { t } = useTranslation();
  const hasRoute = effectiveSourceProtocolBindings(source).some((binding) => binding.modelIds.length > 0);
  return <span className="source-protocol-summary">{t(hasRoute ? "sources.routingSummary" : "sources.routingPending")}</span>;
}

export function SourceProtocolBindingsEditor({ models, value, onChange, wireApis = sourceWireApis,
  showSimplePicker = false, autoAssignModels = true, exclusiveSimplePicker = false, routeGroup = "all",
}: SourceProtocolBindingsEditorProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const [client, setClient] = useState<SourceWireApi>("responses");
  const bindings = normalizedBindings(value, models);
  const selectedModels = (binding: SourceProtocolBinding) => binding.modelIds.length || bindings.length > 1 || normalizedAdapter(binding) !== "native" || !autoAssignModels
    ? binding.modelIds : models;
  const bindingFor = (wireApi: SourceWireApi, adapter: SourceAdapter) => bindings.find((binding) => binding.wireApi === wireApi && normalizedAdapter(binding) === adapter);
  const cacheBindings = bindings.filter((binding) => upstreamWireApi(binding) === "messages");
  const cacheWriteTtl: CacheWriteTtl = cacheBindings.find((binding) => binding.cacheWriteTtl === "1h" || binding.cacheWriteTtl === "5m")?.cacheWriteTtl ?? "provider";
  const simplePicker = <section className={`source-protocol-simple${exclusiveSimplePicker ? " exclusive" : ""}`} aria-labelledby={titleId}>
    <header><strong id={titleId}>{t("sources.simpleRouteTitle")}</strong></header>
    <div className="source-route-simple-options" role="radiogroup" aria-label={t("sources.simpleRouteTitle")}>
      {simpleRouteCards.filter((card) => wireApis.includes(card.wireApi)).map((card) => {
        const Icon = card.icon;
        const first = bindings[0];
        const selected = bindings.length === 1 && first != null && first.wireApi === card.wireApi && normalizedAdapter(first) === card.adapter;
        return <button key={card.id} type="button" role="radio" aria-checked={selected} className={selected ? "selected" : ""}
          onClick={() => onChange([{ wireApi: card.wireApi, adapter: card.adapter, reasoningMode: "disabled", cacheWriteTtl: "provider", modelIds: autoAssignModels ? [...models] : [] }])}>
          <Icon aria-hidden /><span><strong>{t(`sources.protocolCards.${card.wireApi}.title`)}</strong><small>{protocolPresentation[card.wireApi].endpoint}</small></span>
        </button>;
      })}
    </div>
  </section>;
  if (!models.length || (exclusiveSimplePicker && showSimplePicker)) return <section className="source-protocol-bindings">{simplePicker}</section>;
  const columns = wireApis.filter((upstream) => routeGroup !== "adapters" || upstream !== client).map((upstream) => ({
    upstream, wireApi: routeGroup === "native" ? upstream : client,
    adapter: routeGroup === "native" ? "native" as const : adapterBetween(client, upstream),
  }));
  return <section className="source-protocol-bindings" aria-label={t("sources.editorRoutesTab")}>
    {showSimplePicker ? simplePicker : null}
    {routeGroup !== "native" ? <div className="source-client-protocol"><span>{t("sources.clientProtocol")}</span>
      <Tabs value={client} items={wireApis.map((wireApi) => ({ id: wireApi, label: t(`sources.protocolCards.${wireApi}.title`) }))}
        onChange={(wireApi) => setClient(wireApi as SourceWireApi)} label={t("sources.clientProtocol")} />
    </div> : null}
    {cacheBindings.length ? <div className="source-cache-settings"><strong>{t("sources.cacheWriteTtl")}</strong>
      <OptionMenu className="field-option-menu" label={t("sources.cacheWriteTtl")} value={cacheWriteTtl}
        onChange={(ttl) => onChange(updateCacheWriteTtl(bindings, ttl as CacheWriteTtl))}
        options={(["provider", "5m", "1h"] as const).map((ttl) => ({ value: ttl, label: t(`sources.cacheWriteTtls.${ttl}`) }))} />
    </div> : null}
    <div className="source-route-matrix" style={{ "--source-route-column-count": columns.length } as CSSProperties}>
      <div className="source-route-matrix-heading"><span>{t("sources.modelColumn")}</span><div className="source-route-format-headings">
        {columns.map(({ upstream, wireApi, adapter }) => {
          const Icon = protocolPresentation[upstream].icon;
          const binding = bindingFor(wireApi, adapter);
          const assigned = binding ? selectedModels(binding).length : 0;
          return <label key={upstream} className={`source-route-format-heading ${assigned ? "selected" : ""}`} data-wire-api={upstream} data-relay-tooltip={`POST ${protocolPresentation[upstream].endpoint}`}>
            <span className="source-route-format-icon" aria-hidden><Icon /></span><strong>{t(`sources.protocolCards.${upstream}.title`)}</strong>
            {routeGroup === "native" ? <input type="checkbox" checked={assigned > 0} ref={(element) => { if (element) element.indeterminate = assigned > 0 && assigned < models.length; }}
              aria-label={t("sources.protocolAvailableControl", { protocol: t(`sources.protocolCards.${upstream}.title`) })}
              onChange={(event) => onChange(updateNativeProtocol({ bindings, models, autoAssignModels, wireApi, selected: event.target.checked }))} /> : null}
          </label>;
        })}
      </div></div>
      <div className="source-route-model-list">{models.map((model) => <div key={model} className="source-route-model-row">
        <code className="source-route-model-name">{model}</code><div className="source-route-model-controls">
          {columns.map(({ upstream, wireApi, adapter }) => {
            const binding = bindingFor(wireApi, adapter);
            const checked = Boolean(binding && selectedModels(binding).some((id) => id.toLowerCase() === model.toLowerCase()));
            const lastRoute = checked && bindings.length === 1 && binding && selectedModels(binding).length === 1;
            return <label key={upstream} className={`source-route-cell ${checked ? "selected" : ""}`}
              tabIndex={lastRoute ? 0 : undefined} data-relay-tooltip={lastRoute ? t("sources.modelRouteRequired") : undefined}>
              <span className="source-route-cell-label" aria-hidden>{t(`sources.protocolCards.${upstream}.title`)}</span>
              <input type="checkbox" checked={checked} disabled={Boolean(lastRoute)}
                aria-label={t("sources.modelProtocolControl", { model, protocol: routeGroup === "native" ? t(`sources.protocolCards.${upstream}.title`) : `${t(`sources.protocolCards.${client}.title`)} → ${t(`sources.protocolCards.${upstream}.title`)}` })}
                onChange={(event) => onChange(updateModelRoute({ bindings, models, autoAssignModels, wireApi, adapter, model, selected: event.target.checked }))} />
            </label>;
          })}
        </div></div>)}</div>
    </div>
  </section>;
}
