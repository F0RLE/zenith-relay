import { Braces, MessageSquareText, Route, Sparkles } from "lucide-react";
import { type CSSProperties, useId } from "react";
import { useTranslation } from "react-i18next";
import type {
  MessagesReasoningMode,
  SourceAdapter,
  SourceProtocolBinding,
  SourceSummary,
  SourceWireApi,
} from "../api/types";

export const sourceWireApis: SourceWireApi[] = [
  "responses",
  "messages",
  "chat_completions",
];

const protocolPresentation = {
  responses: { icon: Sparkles, endpoint: "/responses" },
  messages: { icon: MessageSquareText, endpoint: "/messages" },
  chat_completions: { icon: Braces, endpoint: "/chat/completions" },
} as const;

function isSourceWireApi(value: string): value is SourceWireApi {
  return sourceWireApis.includes(value as SourceWireApi);
}

const supportedMessagesReasoningModes: readonly MessagesReasoningMode[] = [
  "disabled",
  "budget",
  "adaptive",
];

function normalizedAdapter(binding: SourceProtocolBinding): SourceAdapter {
  return binding.adapter === "responses_to_messages" && binding.wireApi === "responses"
    ? binding.adapter
    : "native";
}

function normalizedReasoningMode(
  binding: SourceProtocolBinding,
  adapter = normalizedAdapter(binding),
): MessagesReasoningMode {
  return adapter === "responses_to_messages"
    && supportedMessagesReasoningModes.includes(binding.reasoningMode ?? "disabled")
    ? binding.reasoningMode ?? "disabled"
    : "disabled";
}

function normalizedModelIds(modelIds: string[], availableModels: string[]) {
  const knownModels = new Map(
    availableModels.map((model) => [model.toLowerCase(), model] as const),
  );
  const seen = new Set<string>();
  return modelIds.flatMap((model) => {
    const normalized = model.trim().toLowerCase();
    const known = knownModels.get(normalized);
    if (!known || seen.has(normalized)) return [];
    seen.add(normalized);
    return [known];
  });
}

function normalizedBindings(bindings: SourceProtocolBinding[], availableModels: string[]) {
  const seen = new Set<string>();
  return bindings.flatMap((binding) => {
    const adapter = normalizedAdapter(binding);
    const routeKey = `${binding.wireApi}:${adapter}`;
    if (!isSourceWireApi(binding.wireApi) || seen.has(routeKey)) return [];
    seen.add(routeKey);
    const modelIds = binding.modelIds.length
      ? normalizedModelIds(binding.modelIds, availableModels)
      : [];
    return [{
      wireApi: binding.wireApi,
      modelIds,
      adapter,
      reasoningMode: normalizedReasoningMode(binding, adapter),
    }];
  });
}

/**
 * Legacy source records keep a single `wireApi`. Treat them as one virtual
 * binding in the UI so an edit never has to guess a protocol from a provider
 * name or silently widen the source's surface.
 */
export function effectiveSourceProtocolBindings(
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">,
): SourceProtocolBinding[] {
  const configured = source.protocolBindings?.length
    ? normalizedBindings(source.protocolBindings, source.models)
    : [];
  return configured.length
    ? configured
    : [{ wireApi: source.wireApi, modelIds: [...source.models] }];
}

export function sourceSupportsWireApi(
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">,
  wireApi: SourceWireApi,
) {
  return effectiveSourceProtocolBindings(source).some(
    (binding) => binding.wireApi === wireApi && binding.modelIds.length > 0,
  );
}

/**
 * A direct ChatGPT profile bypasses Relay entirely. It can use a real
 * Responses endpoint, but it cannot execute a Relay-owned bridge.
 */
export function sourceSupportsNativeResponses(
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">,
) {
  return effectiveSourceProtocolBindings(source).some(
    (binding) =>
      binding.wireApi === "responses"
      && normalizedAdapter(binding) === "native"
      && binding.modelIds.length > 0,
  );
}

export function SourceProtocolBindingsSummary({
  source,
}: {
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">;
}) {
  const { t } = useTranslation();
  const hasRoute = effectiveSourceProtocolBindings(source).some(
    (binding) => binding.modelIds.length > 0,
  );
  return (
    <span className="source-protocol-summary">
      {t(hasRoute ? "sources.routingSummary" : "sources.routingPending")}
    </span>
  );
}

/**
 * The route table is Relay configuration, not a normal setup choice. Keep the
 * declared capabilities visible only when an operator needs to verify an
 * unusual source, while the default flow remains "connect source, pick model".
 */
export function SourceProtocolRoutingDisclosure({
  models,
  value,
  onChange,
  wireApis = sourceWireApis,
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
}) {
  const { t } = useTranslation();
  const titleId = useId();
  return (
    <section className="source-routing-disclosure" aria-labelledby={titleId}>
      <div className="source-routing-overview">
        <span className="source-routing-icon" aria-hidden="true"><Route /></span>
        <span>
          <strong id={titleId}>{t("sources.routingTitle")}</strong>
          <small>{t("sources.modelRoutingHint")}</small>
        </span>
      </div>
      <details className="source-routing-details">
        <summary>
          <span>{t("sources.routingAdvanced")}</span>
          <small>{t("sources.routingAdvancedHint")}</small>
        </summary>
        <SourceProtocolBindingsEditor
          models={models}
          value={value}
          onChange={onChange}
          wireApis={wireApis}
        />
      </details>
    </section>
  );
}

export function SourceProtocolBindingsEditor({
  models,
  value,
  onChange,
  wireApis = sourceWireApis,
}: {
  models: string[];
  value: SourceProtocolBinding[];
  onChange: (value: SourceProtocolBinding[]) => void;
  wireApis?: readonly SourceWireApi[];
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const bindings = normalizedBindings(value, models)
    .filter((binding) => wireApis.includes(binding.wireApi));
  const routeBinding = (wireApi: SourceWireApi, adapter: SourceAdapter) =>
    bindings.find(
      (binding) => binding.wireApi === wireApi && normalizedAdapter(binding) === adapter,
    );
  const hasNativeProtocol = (wireApi: SourceWireApi) =>
    Boolean(routeBinding(wireApi, "native"));
  const nativeResponsesBinding = routeBinding("responses", "native");
  const bridgeBinding = routeBinding("responses", "responses_to_messages");
  const messagesBinding = routeBinding("messages", "native");
  const hasMultipleRoutes = bindings.length > 1;
  const selectedModels = (binding: SourceProtocolBinding) =>
    binding.modelIds.length || hasMultipleRoutes ? binding.modelIds : models;
  const modelIsSelected = (binding: SourceProtocolBinding | undefined, model: string) =>
    Boolean(binding && selectedModels(binding).some(
      (candidate) => candidate.toLowerCase() === model.toLowerCase(),
    ));
  const bridgeModels = bridgeBinding ? selectedModels(bridgeBinding) : [];
  const showsBridgeColumn = wireApis.includes("messages") || Boolean(bridgeBinding);
  const matrixStyle = {
    "--source-route-column-count": String(wireApis.length + (showsBridgeColumn ? 1 : 0)),
  } as CSSProperties;

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
    if (routeBinding(wireApi, "native")) return;
    onChange([
      ...bindings,
      {
        wireApi,
        // A second route is intentionally unassigned until its documented
        // capability is verified. Filling it with every discovered model
        // would advertise a protocol the source may not support.
        modelIds: bindings.length ? [] : [...models],
        adapter: "native",
        reasoningMode: "disabled",
      },
    ]);
  };

  const setModel = (
    wireApi: SourceWireApi,
    adapter: SourceAdapter,
    model: string,
    selected: boolean,
  ) => {
    const nextBindings = bindings.map((binding) => {
      if (binding.wireApi !== wireApi || normalizedAdapter(binding) !== adapter) return binding;
      const selectedIds = selectedModels(binding);
      const modelKey = model.toLowerCase();
      const nextModelIds = selected
        ? [...selectedIds, model]
        : selectedIds.filter((candidate) => candidate.toLowerCase() !== modelKey);
      // A single legacy binding may use an empty list as its source-wide
      // fallback. With two routes, however, an empty list means an
      // intentionally unassigned route so a model can be moved safely.
      return {
        ...binding,
        modelIds: nextModelIds.length || hasMultipleRoutes
          ? normalizedModelIds(nextModelIds, models)
          : selectedIds,
      };
    });
    onChange(nextBindings);
  };

  const setBridgeModel = (model: string, selected: boolean) => {
    if (!selected) {
      if (!bridgeBinding) return;
      const nextModelIds = normalizedModelIds(
        bridgeModels.filter((candidate) => candidate.toLowerCase() !== model.toLowerCase()),
        models,
      );
      onChange(nextModelIds.length
        ? bindings.map((binding) => (
          binding === bridgeBinding
            ? { ...binding, modelIds: nextModelIds }
            : binding
        ))
        : bindings.filter((binding) => binding !== bridgeBinding));
      return;
    }

    // A bridged route is still a native Messages capability upstream. Keeping
    // the relationship in one operation prevents the UI from advertising a
    // Responses route whose upstream Messages model was not declared.
    if (!messagesBinding || modelIsSelected(nativeResponsesBinding, model)) return;
    const nextMessageModels = normalizedModelIds(
      [...selectedModels(messagesBinding), model],
      models,
    );
    const nextBindings = bindings.map((binding) => (
      binding === messagesBinding
        ? { ...binding, modelIds: nextMessageModels }
        : binding
    ));
    if (bridgeBinding) {
      onChange(nextBindings.map((binding) => (
        binding === bridgeBinding
          ? {
            ...binding,
            modelIds: normalizedModelIds([...bridgeModels, model], models),
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
        reasoningMode: "disabled",
        modelIds: [model],
      },
    ]);
  };

  return (
    <section className="source-protocol-bindings" aria-labelledby={titleId}>
      <header>
        <strong id={titleId}>{t("sources.protocolsTitle")}</strong>
        <p>{t("sources.protocolsHint")}</p>
      </header>
      <div className="source-route-matrix" style={matrixStyle}>
          <div className="source-route-matrix-heading">
            <span>{t("sources.modelColumn")}</span>
            <div className="source-route-format-headings">
              {wireApis.map((wireApi) => {
                const selected = hasNativeProtocol(wireApi);
                const { icon: Icon, endpoint } = protocolPresentation[wireApi];
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
                      <small>{t(`sources.protocolCards.${wireApi}.hint`)}</small>
                    </span>
                    <input
                      type="checkbox"
                      checked={selected}
                      aria-label={t("sources.protocolAvailableControl", {
                        protocol: t(`sources.protocolCards.${wireApi}.title`),
                      })}
                      onChange={(event) => setNativeProtocol(wireApi, event.target.checked)}
                    />
                  </label>
                );
              })}
              {showsBridgeColumn
                ? <div className="source-route-bridge-heading">
                  <span className="source-route-format-icon" aria-hidden="true"><Route /></span>
                  <span>
                    <strong>{t("sources.bridgeColumnTitle")}</strong>
                    <small>{t("sources.bridgeColumnHint")}</small>
                  </span>
                </div>
                : null}
            </div>
          </div>
          {models.length
            ? <div className="source-route-model-list">
              {models.map((model) => {
              const bridgeChecked = modelIsSelected(bridgeBinding, model);
              const directResponsesChecked = modelIsSelected(nativeResponsesBinding, model);
              const bridgeIsLastAvailableRoute = bridgeChecked
                && bridgeModels.length === 1
                && bindings.length === 1;
              const bridgeDisabled = bridgeIsLastAvailableRoute
                || (!bridgeChecked && (!messagesBinding || directResponsesChecked));
              const bridgeTitle = bridgeIsLastAvailableRoute
                ? t("sources.modelRouteRequired")
                : !bridgeChecked && !messagesBinding
                  ? t("sources.bridgeRequiresMessages")
                  : !bridgeChecked && directResponsesChecked
                    ? t("sources.bridgeRouteConflict")
                    : undefined;
              return (
                <div key={model} className="source-route-model-row">
                  <code className="source-route-model-name">{model}</code>
                  <div className="source-route-model-controls">
                    {wireApis.map((wireApi) => {
                      const binding = routeBinding(wireApi, "native");
                      const checked = modelIsSelected(binding, model);
                      const assignedToOtherRoute = Boolean(binding) && bindings.some(
                        (candidate) =>
                          candidate.wireApi === wireApi
                          && normalizedAdapter(candidate) !== "native"
                          && modelIsSelected(candidate, model),
                      );
                      const requiredByBridge = wireApi === "messages"
                        && bridgeChecked;
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
                    {showsBridgeColumn
                      ? <label
                        className={`source-route-cell source-route-bridge-cell ${bridgeChecked ? "selected" : ""}`}
                        title={bridgeTitle}
                      >
                        <span className="source-route-cell-label" aria-hidden="true">
                          {t("sources.bridgeColumnTitle")}
                        </span>
                        <input
                          type="checkbox"
                          checked={bridgeChecked}
                          disabled={bridgeDisabled}
                          aria-label={t("sources.modelBridgeControl", { model })}
                          onChange={(event) => setBridgeModel(model, event.target.checked)}
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
      {showsBridgeColumn
        ? <p className="source-route-bridge-note">{t("sources.bridgeHint")}</p>
        : null}
    </section>
  );
}
