import { Braces, Globe2, MessageSquareText, Route, Sparkles, type LucideIcon } from "lucide-react";
import { type CSSProperties, useId } from "react";
import { useTranslation } from "react-i18next";
import type { CacheWriteTtl, SourceAdapter, SourceProtocolBinding, SourceWireApi } from "../api/types";
import {
  normalizedAdapter,
  normalizedBindings,
  normalizedModelIds,
  sourceWireApis,
} from "../sourceProtocolBindings";
import { OptionMenu } from "./Ui";

const protocolPresentation = {
  responses: { icon: Sparkles, endpoint: "/responses" },
  messages: { icon: MessageSquareText, endpoint: "/messages" },
  chat_completions: { icon: Braces, endpoint: "/chat/completions" },
  gemini: { icon: Globe2, endpoint: "/v1beta/models/{model}:generateContent" },
} as const;

type SimpleRouteCard = {
  id: string;
  wireApi: SourceWireApi;
  adapter: SourceAdapter;
  icon: LucideIcon;
  titleKey: string;
  subtitleKey: string;
};

const simpleRouteCards: readonly SimpleRouteCard[] = [
  { id: "openai", wireApi: "responses", adapter: "native", icon: Sparkles, titleKey: "sources.simpleRouteCards.openai.title", subtitleKey: "sources.simpleRouteCards.openai.protocol" },
  { id: "anthropic", wireApi: "messages", adapter: "native", icon: MessageSquareText, titleKey: "sources.simpleRouteCards.anthropic.title", subtitleKey: "sources.simpleRouteCards.anthropic.protocol" },
  // Google sources use the provider's native Gemini contract by default. The
  // Responses-to-Gemini bridge remains available in the advanced route matrix.
  { id: "google", wireApi: "gemini", adapter: "native", icon: Globe2, titleKey: "sources.simpleRouteCards.google.title", subtitleKey: "sources.simpleRouteCards.google.protocol" },
] as const;

export function SourceProtocolBindingsEditor({
  models,
  value,
  onChange,
  wireApis = sourceWireApis,
  showSimplePicker = false,
  autoAssignModels = true,
  exclusiveSimplePicker = false,
  routeGroup = "all",
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
  showSimplePicker?: boolean;
  autoAssignModels?: boolean;
  /** Keep the setup flow to one adapter for the whole model catalog. */
  exclusiveSimplePicker?: boolean;
  /**
   * Render native provider formats and Relay adapters together, or restrict the
   * matrix to one of them when the dialog splits them into separate tabs.
   */
  routeGroup?: "all" | "native" | "adapters";
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const bindings = normalizedBindings(value, models)
    .filter((binding) => wireApis.includes(binding.wireApi));
  const routeBinding = (wireApi: SourceWireApi, adapter: SourceAdapter) =>
    bindings.find(
      (binding) => binding.wireApi === wireApi && normalizedAdapter(binding) === adapter,
    );
  const nativeResponsesBinding = routeBinding("responses", "native");
  const messagesBridgeBinding = routeBinding("responses", "responses_to_messages");
  const geminiBridgeBinding = routeBinding("responses", "responses_to_gemini");
  const cacheBindings = bindings.filter((binding) => (
    binding.wireApi === "messages" || normalizedAdapter(binding) === "responses_to_messages"
  ));
  const cacheWriteTtl: CacheWriteTtl = cacheBindings.some((binding) => binding.cacheWriteTtl === "1h")
    ? "1h"
    : cacheBindings.some((binding) => binding.cacheWriteTtl === "5m")
      ? "5m"
      : "provider";
  const hasMultipleRoutes = bindings.length > 1;
  const selectedModels = (binding: SourceProtocolBinding) =>
    binding.modelIds.length || hasMultipleRoutes || normalizedAdapter(binding) !== "native" || !autoAssignModels
      ? binding.modelIds
      : models;
  const nativeProtocolModels = (wireApi: SourceWireApi) => {
    const binding = routeBinding(wireApi, "native");
    return binding ? selectedModels(binding) : [];
  };
  const nativeProtocolState = (wireApi: SourceWireApi) => {
    const assignedCount = nativeProtocolModels(wireApi).length;
    return {
      assignedCount,
      selected: assignedCount > 0,
      partial: assignedCount > 0 && assignedCount < models.length,
    };
  };
  const routesMayShareModel = (
    left: SourceProtocolBinding,
    right: SourceProtocolBinding,
  ) => (
    (left.wireApi === "messages"
      && normalizedAdapter(left) === "native"
      && right.wireApi === "responses"
      && normalizedAdapter(right) === "responses_to_messages")
    || (right.wireApi === "messages"
      && normalizedAdapter(right) === "native"
      && left.wireApi === "responses"
      && normalizedAdapter(left) === "responses_to_messages")
  );
  const removeModelFromOtherRoutes = (
    sourceBindings: SourceProtocolBinding[],
    target: SourceProtocolBinding,
    model: string,
  ) => sourceBindings.map((binding) => {
    if (binding === target || routesMayShareModel(binding, target)) return binding;
    const selectedIds = selectedModels(binding);
    const nextModelIds = selectedIds.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase());
    return {
      ...binding,
      // Materialize the remaining catalog when a legacy single-route binding
      // was using an empty model list as its source-wide fallback.
      modelIds: normalizedModelIds(nextModelIds, models),
    };
  });
  const addModelToRoute = (
    sourceBindings: SourceProtocolBinding[],
    target: SourceProtocolBinding,
    model: string,
  ) => {
    const selectedIds = selectedModels(target);
    return sourceBindings.map((binding) => binding === target
      ? { ...binding, modelIds: normalizedModelIds([...selectedIds, model], models) }
      : binding);
  };
  const modelIsSelected = (binding: SourceProtocolBinding | undefined, model: string) =>
    Boolean(binding && selectedModels(binding).some(
      (candidate) => candidate.toLowerCase() === model.toLowerCase(),
    ));
  // Bridge routes are explicit. A native Messages route does not silently
  // become a Responses route and is never rendered as "Auto".
  const messagesBridgeModels = messagesBridgeBinding
    ? selectedModels(messagesBridgeBinding)
    : [];
  const geminiBridgeModels = geminiBridgeBinding ? selectedModels(geminiBridgeBinding) : [];
  const nativeWireApis = routeGroup === "adapters" ? [] : wireApis;
  const showsMessagesBridgeColumn = routeGroup !== "native";
  const showsGeminiBridgeColumn = routeGroup !== "native";
  const showsGroupHeadings = routeGroup === "all";
  // The cache control belongs to whichever route group is currently visible:
  // native Messages on the formats tab, the Messages bridge on the adapters tab.
  const visibleCacheBindings = cacheBindings.filter((binding) => (
    normalizedAdapter(binding) === "native"
      ? nativeWireApis.includes(binding.wireApi)
      : showsMessagesBridgeColumn
  ));
  const matrixStyle = {
    "--source-route-column-count": String(
      nativeWireApis.length + Number(showsMessagesBridgeColumn) + Number(showsGeminiBridgeColumn),
    ),
  } as CSSProperties;

  const renderSimplePicker = (cards: readonly SimpleRouteCard[], exclusive = false) => {
    const selectedCard = cards.find((card) => {
      const binding = bindings.find((candidate) => candidate.wireApi === card.wireApi);
      return binding && normalizedAdapter(binding) === card.adapter;
    });
    return (
    <section className={`source-protocol-simple${exclusive ? " exclusive" : ""}`} aria-labelledby={titleId}>
      <header>
        <strong id={titleId}>{t("sources.simpleRouteTitle")}</strong>
      </header>
      <div className="source-route-simple-options" role="radiogroup" aria-label={t("sources.simpleRouteTitle")}>
        {cards.map((card) => {
          const Icon = card.icon;
          const selected = selectedCard?.id === card.id;
          return (
            <button
              key={card.id}
              type="button"
              role="radio"
              aria-checked={selected}
              className={selected ? "selected" : ""}
              onClick={() => onChange([{
                wireApi: card.wireApi,
                adapter: card.adapter,
                reasoningMode: "disabled",
                cacheWriteTtl: card.wireApi === "messages" ? "1h" : "provider",
                modelIds: autoAssignModels ? [...models] : [],
              }])}
            >
              <Icon aria-hidden />
              <span>
                <strong>{t(card.titleKey)}</strong>
                <small>{t(card.subtitleKey)}</small>
              </span>
            </button>
          );
        })}
      </div>
    </section>
    );
  };
  const simplePicker = renderSimplePicker(simpleRouteCards);
  const exclusivePicker = renderSimplePicker(simpleRouteCards, true);

  if (!models.length) {
    return <section className="source-protocol-bindings">{exclusiveSimplePicker && showSimplePicker ? exclusivePicker : simplePicker}</section>;
  }

  if (exclusiveSimplePicker && showSimplePicker) {
    return <section className="source-protocol-bindings">{exclusivePicker}</section>;
  }

  const setNativeProtocol = (wireApi: SourceWireApi, selected: boolean) => {
    if (!selected) {
      onChange(bindings.filter((binding) => !(
        binding.wireApi === wireApi && normalizedAdapter(binding) === "native"
      )));
      return;
    }
    const existing = routeBinding(wireApi, "native");
    if (existing) {
      const nextModels = normalizedModelIds([...selectedModels(existing), ...models], models);
      onChange(bindings.map((binding) => binding === existing
        ? { ...binding, modelIds: nextModels }
        : binding));
      return;
    }
    const target = existing ?? {
      wireApi,
      // A secondary format starts unassigned. Its model cells become active
      // immediately, while the header remains off until a model is routed.
      modelIds: bindings.length ? [] : [...models],
      adapter: "native" as const,
      reasoningMode: "disabled" as const,
      cacheWriteTtl: wireApi === "messages" ? "1h" as const : "provider" as const,
    };
    const withTarget = existing ? bindings : [...bindings, target];
    onChange(withTarget);
  };

  const setModel = (
    wireApi: SourceWireApi,
    adapter: SourceAdapter,
    model: string,
    selected: boolean,
  ) => {
    const target = routeBinding(wireApi, adapter);
    if (!target) return;
    if (!selected) {
      const selectedIds = selectedModels(target);
      const nextModelIds = selectedIds.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase());
      if (!nextModelIds.length && hasMultipleRoutes) {
        onChange(bindings.filter((binding) => binding !== target));
        return;
      }
      onChange(bindings.map((binding) => binding === target
        ? {
          ...binding,
          // A single legacy binding may use an empty list as its source-wide
          // fallback. Keep the final route intact so the source cannot become
          // unroutable through a model checkbox.
          modelIds: nextModelIds.length || hasMultipleRoutes
            ? normalizedModelIds(nextModelIds, models)
            : selectedIds,
        }
        : binding));
      return;
    }
    const moved = removeModelFromOtherRoutes(bindings, target, model);
    const nextBindings = addModelToRoute(moved, target, model);
    onChange(nextBindings);
  };

  const setMessagesBridgeModel = (model: string, selected: boolean) => {
    if (!selected) {
      if (!messagesBridgeBinding) return;
      const nextModelIds = normalizedModelIds(
        messagesBridgeModels.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
        models,
      );
      onChange(nextModelIds.length
        ? bindings.map((binding) => (
          binding === messagesBridgeBinding
            ? { ...binding, modelIds: nextModelIds }
            : binding
        ))
        : bindings.filter((binding) => binding !== messagesBridgeBinding));
      return;
    }

    if (messagesBridgeBinding) {
      const moved = removeModelFromOtherRoutes(bindings, messagesBridgeBinding, model);
      onChange(moved.map((binding) => (
        binding === messagesBridgeBinding
          ? {
            ...binding,
            modelIds: normalizedModelIds([...messagesBridgeModels, model], models),
          }
          : binding
      )));
      return;
    }
    onChange([
      ...removeModelFromOtherRoutes(bindings, {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
        cacheWriteTtl: "1h",
        modelIds: [model],
      }, model),
      {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
        cacheWriteTtl: cacheWriteTtl === "provider" ? "1h" : cacheWriteTtl,
        modelIds: [model],
      },
    ]);
  };
  const setGeminiBridgeModel = (model: string, selected: boolean) => {
    if (!selected) {
      if (!geminiBridgeBinding) return;
      const nextModelIds = normalizedModelIds(
        geminiBridgeModels.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
        models,
      );
      onChange(nextModelIds.length
        ? bindings.map((binding) => (
          binding === geminiBridgeBinding
            ? { ...binding, modelIds: nextModelIds }
            : binding
        ))
        : bindings.filter((binding) => binding !== geminiBridgeBinding));
      return;
    }

    if (geminiBridgeBinding) {
      const moved = removeModelFromOtherRoutes(bindings, geminiBridgeBinding, model);
      onChange(addModelToRoute(moved, geminiBridgeBinding, model).map((binding) => (
        binding === geminiBridgeBinding
          ? { ...binding, modelIds: normalizedModelIds([...geminiBridgeModels, model], models) }
          : binding
      )));
      return;
    }
    const target: SourceProtocolBinding = {
      wireApi: "responses",
      adapter: "responses_to_gemini",
      reasoningMode: "disabled",
      modelIds: [model],
    };
    onChange([
      ...removeModelFromOtherRoutes(bindings, target, model),
      target,
    ]);
  };
  const setCacheWriteTtl = (cacheWriteTtl: CacheWriteTtl) => {
    onChange(bindings.map((binding) => (
      binding.wireApi === "messages" || normalizedAdapter(binding) === "responses_to_messages"
        ? { ...binding, cacheWriteTtl }
        : binding
    )));
  };
  return (
    <section className="source-protocol-bindings" aria-labelledby={titleId}>
      {showSimplePicker ? simplePicker : null}
      {visibleCacheBindings.length ? <div className="source-cache-settings">
        <strong>{t("sources.cacheWriteTtl")}</strong>
        <OptionMenu
          className="field-option-menu"
          label={t("sources.cacheWriteTtl")}
          value={cacheWriteTtl}
          onChange={(value) => setCacheWriteTtl(value as CacheWriteTtl)}
          options={[
            { value: "provider", label: t("sources.cacheWriteTtls.provider") },
            { value: "5m", label: t("sources.cacheWriteTtls.5m") },
            { value: "1h", label: t("sources.cacheWriteTtls.1h") },
          ]}
        />
      </div> : null}
      <div className={`source-route-matrix${showsGroupHeadings ? " grouped" : ""}`} style={matrixStyle}>
        <div className="source-route-matrix-heading">
          <span>{t("sources.modelColumn")}</span>
          <div className="source-route-format-headings">
            {showsGroupHeadings ? <span
              className="source-route-group-heading native"
              style={{ "--source-route-group-span": nativeWireApis.length } as CSSProperties}
            >
              {t("sources.nativeRoutesTitle")}
            </span> : null}
            {showsGroupHeadings ? <span className="source-route-group-heading adapters">
              {t("sources.adapterRoutesTitle")}
            </span> : null}
            {nativeWireApis.map((wireApi) => {
              const { icon: Icon, endpoint } = protocolPresentation[wireApi];
              const { selected, partial } = nativeProtocolState(wireApi);
              return (
                <label
                  key={wireApi}
                  className={`source-route-format-heading ${selected ? "selected" : ""}`}
                  data-wire-api={wireApi}
                  title={`POST ${endpoint}`}
                >
                  <span className="source-route-format-icon" aria-hidden="true"><Icon /></span>
                  <span>
                    <strong>{t(`sources.protocolCards.${wireApi}.title`)}</strong>
                  </span>
                  <input
                    type="checkbox"
                    checked={selected}
                    ref={(element) => {
                      if (element) element.indeterminate = partial;
                    }}
                    aria-checked={partial ? "mixed" : selected}
                    aria-label={t("sources.protocolAvailableControl", {
                      protocol: t(`sources.protocolCards.${wireApi}.title`),
                    })}
                    onChange={(event) => setNativeProtocol(wireApi, event.target.checked)}
                  />
                </label>
              );
            })}
             {showsMessagesBridgeColumn
               ? <div className={`source-route-bridge-heading ${messagesBridgeBinding ? "configured" : ""}`}>
                 <span className="source-route-format-icon" aria-hidden="true"><Route /></span>
                 <span>
                   <strong>{t("sources.routeBridgeMessagesTitle")}</strong>
                  </span>
               </div>
               : null}
            {showsGeminiBridgeColumn
              ? <div className="source-route-bridge-heading">
                <span className="source-route-format-icon" aria-hidden="true"><Sparkles /></span>
                <span>
                  <strong>{t("sources.routeBridgeGeminiTitle")}</strong>
                </span>
              </div>
              : null}
          </div>
        </div>
        {models.length
          ? <div className="source-route-model-list">
            {models.map((model) => {
              const explicitMessagesBridgeChecked = modelIsSelected(messagesBridgeBinding, model);
              const geminiBridgeChecked = modelIsSelected(geminiBridgeBinding, model);
              const directResponsesChecked = modelIsSelected(nativeResponsesBinding, model);
              const messagesBridgeChecked = explicitMessagesBridgeChecked;
              const messagesBridgeIsLastAvailableRoute = explicitMessagesBridgeChecked
                && messagesBridgeModels.length === 1
                && bindings.length === 1;
              const messagesBridgeDisabled = messagesBridgeIsLastAvailableRoute
                || (!messagesBridgeChecked && (directResponsesChecked || geminiBridgeChecked));
              const messagesBridgeTitle = messagesBridgeIsLastAvailableRoute
                  ? t("sources.modelRouteRequired")
                  : undefined;
              const geminiBridgeIsLastAvailableRoute = geminiBridgeChecked
                && geminiBridgeModels.length === 1
                && bindings.length === 1;
              const geminiBridgeDisabled = geminiBridgeIsLastAvailableRoute
                || (!geminiBridgeChecked && (directResponsesChecked || messagesBridgeChecked));
              const geminiBridgeTitle = geminiBridgeIsLastAvailableRoute
                ? t("sources.modelRouteRequired")
                : !geminiBridgeChecked && (directResponsesChecked || messagesBridgeChecked)
                  ? t("sources.geminiBridgeRouteConflict")
                  : undefined;
              return (
                <div key={model} className="source-route-model-row">
                  <code className="source-route-model-name">{model}</code>
                  <div className="source-route-model-controls">
                    {nativeWireApis.map((wireApi) => {
                      const binding = routeBinding(wireApi, "native");
                      const checked = modelIsSelected(binding, model);
                      const assignedToOtherRoute = Boolean(binding) && bindings.some(
                        (candidate) => candidate.wireApi === wireApi
                          && normalizedAdapter(candidate) !== "native"
                          && modelIsSelected(candidate, model),
                      );
                      const lastSelectedModel = binding != null
                        && checked
                        && selectedModels(binding).length === 1
                        && bindings.length === 1;
                      const disabled = !binding
                        || lastSelectedModel
                        || (!checked && assignedToOtherRoute);
                      const title = !binding
                        ? t("sources.modelRouteUnavailable")
                          : lastSelectedModel
                            ? t("sources.modelRouteRequired")
                            : !checked && assignedToOtherRoute
                                ? t("sources.bridgeRouteConflict")
                              : undefined;
                      return (
                        <label
                          key={wireApi}
                          className={`source-route-cell ${checked ? "selected" : ""}`}
                          title={title}
                        >
                          <span className="source-route-cell-label" aria-hidden="true">
                            {t(`sources.protocolCards.${wireApi}.title`)}
                          </span>
                          <input
                            type="checkbox"
                            checked={checked}
                            disabled={disabled}
                            aria-label={t("sources.modelProtocolControl", {
                              model,
                              protocol: t(`sources.protocolCards.${wireApi}.title`),
                            })}
                            onChange={(event) => setModel(
                              wireApi,
                              "native",
                              model,
                              event.target.checked,
                            )}
                          />
                        </label>
                      );
                    })}
                    {showsMessagesBridgeColumn
                      ? <label
                          className={`source-route-cell source-route-bridge-cell ${messagesBridgeChecked ? "selected" : ""}`}
                          title={messagesBridgeTitle}
                        >
                          <span className="source-route-cell-label" aria-hidden="true">
                            {t("sources.routeBridgeMessagesTitle")}
                          </span>
                          <input
                            type="checkbox"
                            checked={messagesBridgeChecked}
                            disabled={messagesBridgeDisabled}
                            aria-label={t("sources.modelBridgeControl", { model })}
                            onChange={(event) => setMessagesBridgeModel(model, event.target.checked)}
                          />
                        </label>
                      : null}
                    {showsGeminiBridgeColumn
                      ? <label
                        className={`source-route-cell source-route-bridge-cell ${geminiBridgeChecked ? "selected" : ""}`}
                        title={geminiBridgeTitle}
                      >
                        <span className="source-route-cell-label" aria-hidden="true">
                          {t("sources.routeBridgeGeminiTitle")}
                        </span>
                        <input
                          type="checkbox"
                          checked={geminiBridgeChecked}
                          disabled={geminiBridgeDisabled}
                          aria-label={t("sources.modelGeminiBridgeControl", { model })}
                          onChange={(event) => setGeminiBridgeModel(model, event.target.checked)}
                        />
                      </label>
                      : null}
                  </div>
                </div>
              );
            })}
          </div>
          : null}
      </div>
    </section>
  );
}
