import { Braces, Globe2, Link2, MessageSquareText, Route, Sparkles, type LucideIcon } from "lucide-react";
import { type CSSProperties, useId } from "react";
import { useTranslation } from "react-i18next";
import type { SourceAdapter, SourceProtocolBinding, SourceWireApi } from "../api/types";
import {
  normalizedAdapter,
  normalizedBindings,
  normalizedModelIds,
  sourceWireApis,
} from "../sourceProtocolBindings";

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
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
  showSimplePicker?: boolean;
  autoAssignModels?: boolean;
  /** Keep the setup flow to one adapter for the whole model catalog. */
  exclusiveSimplePicker?: boolean;
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
  const messagesBinding = routeBinding("messages", "native");
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
  const claimedByResponsesRoute = new Set(
    bindings.flatMap((binding) => (
      binding.wireApi === "responses" && normalizedAdapter(binding) !== "responses_to_messages"
        ? selectedModels(binding).map((model) => model.toLowerCase())
        : []
    )),
  );
  const linkedMessagesModels = messagesBinding
    ? selectedModels(messagesBinding).filter((model) => !claimedByResponsesRoute.has(model.toLowerCase()))
    : [];
  const effectiveMessagesBridgeBinding = messagesBridgeBinding ?? (linkedMessagesModels.length
    ? {
      wireApi: "responses" as const,
      adapter: "responses_to_messages" as const,
      reasoningMode: "adaptive" as const,
      modelIds: linkedMessagesModels,
    }
    : undefined);
  const messagesBridgeModels = effectiveMessagesBridgeBinding
    ? normalizedModelIds([
      ...selectedModels(effectiveMessagesBridgeBinding),
      ...(messagesBridgeBinding ? linkedMessagesModels : []),
    ], models)
    : [];
  const geminiBridgeModels = geminiBridgeBinding ? selectedModels(geminiBridgeBinding) : [];
  const messagesBridgeIsAutomatic = !messagesBridgeBinding && linkedMessagesModels.length > 0;
  const showsMessagesBridgeColumn = true;
  const showsGeminiBridgeColumn = true;
  const matrixStyle = {
    "--source-route-column-count": String(
      wireApis.length + Number(showsMessagesBridgeColumn) + Number(showsGeminiBridgeColumn),
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
      onChange(bindings.filter((binding) => (
        wireApi === "messages"
          ? binding.wireApi !== "messages"
            && !(binding.wireApi === "responses" && normalizedAdapter(binding) === "responses_to_messages")
          : !(binding.wireApi === wireApi && normalizedAdapter(binding) === "native")
      )));
      return;
    }
    const existing = routeBinding(wireApi, "native");
    if (existing) {
      const nextModels = normalizedModelIds([...selectedModels(existing), ...models], models);
      onChange(bindings.map((binding) => {
        if (binding === existing) return { ...binding, modelIds: nextModels };
        if (wireApi === "messages"
          && binding.wireApi === "responses"
          && normalizedAdapter(binding) === "responses_to_messages") {
          return { ...binding, modelIds: nextModels };
        }
        return binding;
      }));
      return;
    }
    const target = existing ?? {
      wireApi,
      // A secondary format starts unassigned. Its model cells become active
      // immediately, while the header remains off until a model is routed.
      modelIds: bindings.length ? [] : [...models],
      adapter: "native" as const,
      reasoningMode: "disabled" as const,
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
    onChange(nextBindings.map((binding) => (
      wireApi === "messages"
        && binding.wireApi === "responses"
        && normalizedAdapter(binding) === "responses_to_messages"
        ? { ...binding, modelIds: normalizedModelIds([...selectedModels(binding), model], models) }
        : binding
    )));
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

    // A bridged route is still a native Messages capability upstream. Keeping
    // the relationship in one operation prevents the UI from advertising a
    // Responses route whose upstream Messages model was not declared.
    if (!messagesBinding) return;
    const nextMessageModels = normalizedModelIds(
      [...selectedModels(messagesBinding), model],
      models,
    );
    const nextBindings = removeModelFromOtherRoutes(bindings, messagesBinding, model).map((binding) => (
      binding === messagesBinding
        ? { ...binding, modelIds: nextMessageModels }
        : binding
    ));
    if (messagesBridgeBinding) {
      onChange(nextBindings.map((binding) => (
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
      ...nextBindings,
      {
        wireApi: "responses",
        adapter: "responses_to_messages",
        reasoningMode: "adaptive",
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
  return (
    <section className="source-protocol-bindings" aria-labelledby={titleId}>
      {showSimplePicker ? simplePicker : null}
      <div className="source-route-matrix" style={matrixStyle}>
        <div className="source-route-matrix-heading">
          <span>{t("sources.modelColumn")}</span>
          <div className="source-route-format-headings">
            {wireApis.map((wireApi) => {
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
               ? <div className={`source-route-bridge-heading ${effectiveMessagesBridgeBinding ? "configured" : ""} ${messagesBridgeIsAutomatic ? "automatic" : ""}`}>
                 <span className="source-route-format-icon" aria-hidden="true"><Route /></span>
                 <span>
                   <strong>{t("sources.routeBridgeMessagesTitle")}</strong>
                   {messagesBridgeIsAutomatic ? <small>{t("sources.bridgeAutoLabel")}</small> : null}
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
              const nativeMessagesChecked = modelIsSelected(messagesBinding, model);
              const messagesBridgeLinkedAutomatically = !explicitMessagesBridgeChecked
                && nativeMessagesChecked
                && !directResponsesChecked
                && !geminiBridgeChecked;
              const messagesBridgeChecked = explicitMessagesBridgeChecked
                || (messagesBridgeBinding && messagesBridgeModels.some((candidate) => candidate.toLowerCase() === model.toLowerCase()))
                || messagesBridgeLinkedAutomatically;
              const messagesBridgeIsLastAvailableRoute = explicitMessagesBridgeChecked
                && messagesBridgeModels.length === 1
                && bindings.length === 1;
              const messagesBridgeDisabled = messagesBridgeLinkedAutomatically
                || messagesBridgeIsLastAvailableRoute
                || (!messagesBridgeChecked && (!messagesBinding || directResponsesChecked || geminiBridgeChecked));
              const messagesBridgeTitle = messagesBridgeLinkedAutomatically
                ? t("sources.bridgeLinkedAutomatically")
                : messagesBridgeIsLastAvailableRoute
                  ? t("sources.modelRouteRequired")
                  : !messagesBridgeChecked && !messagesBinding
                    ? t("sources.bridgeRequiresMessages")
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
                    {wireApis.map((wireApi) => {
                      const binding = routeBinding(wireApi, "native");
                      const checked = modelIsSelected(binding, model);
                      const assignedToOtherRoute = Boolean(binding) && bindings.some(
                        (candidate) => candidate.wireApi === wireApi
                          && normalizedAdapter(candidate) !== "native"
                          && modelIsSelected(candidate, model),
                      );
                      const requiredByBridge = wireApi === "messages"
                        && explicitMessagesBridgeChecked;
                      const lastSelectedModel = binding != null
                        && checked
                        && selectedModels(binding).length === 1
                        && bindings.length === 1;
                      const disabled = !binding
                        || lastSelectedModel
                        || (checked && requiredByBridge)
                        || (!checked && assignedToOtherRoute);
                      const title = !binding
                        ? t("sources.modelRouteUnavailable")
                          : lastSelectedModel
                            ? t("sources.modelRouteRequired")
                            : checked && requiredByBridge
                              ? t("sources.bridgeRequiresMessages")
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
                      ? messagesBridgeLinkedAutomatically
                        ? <div
                          className="source-route-cell source-route-bridge-cell source-route-auto-cell selected"
                          role="img"
                          data-automatic="true"
                          aria-label={t("sources.bridgeAutoRoute", { model })}
                          title={messagesBridgeTitle}
                        >
                          <span className="source-route-cell-label">
                            {t("sources.routeBridgeMessagesTitle")}
                          </span>
                          <span className="source-route-auto-indicator" aria-hidden="true">
                            <Link2 />
                            <small>{t("sources.bridgeAutoLabel")}</small>
                          </span>
                        </div>
                        : <label
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
