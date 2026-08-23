import { Braces, MessageSquareText, Plus, Route, Sparkles } from "lucide-react";
import { type CSSProperties, useId } from "react";
import { useTranslation } from "react-i18next";
import type {
  SourceAdapter,
  CacheWriteTtl,
  SourceProtocolBinding,
  SourceWireApi,
} from "../api/types";
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
} as const;

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
  const messagesBridgeBinding = routeBinding("responses", "responses_to_messages");
  const geminiBridgeBinding = routeBinding("responses", "responses_to_gemini");
  const messagesBinding = routeBinding("messages", "native");
  const cacheBindings = bindings.filter((binding) => (
    binding.wireApi === "messages" || normalizedAdapter(binding) === "responses_to_messages"
  ));
  const hasMultipleRoutes = bindings.length > 1;
  const selectedModels = (binding: SourceProtocolBinding) =>
    binding.modelIds.length || hasMultipleRoutes || normalizedAdapter(binding) !== "native"
      ? binding.modelIds
      : models;
  const modelIsSelected = (binding: SourceProtocolBinding | undefined, model: string) =>
    Boolean(binding && selectedModels(binding).some(
      (candidate) => candidate.toLowerCase() === model.toLowerCase(),
    ));
  const messagesBridgeModels = messagesBridgeBinding ? selectedModels(messagesBridgeBinding) : [];
  const geminiBridgeModels = geminiBridgeBinding ? selectedModels(geminiBridgeBinding) : [];
  const activeWireApis = wireApis.filter(hasNativeProtocol);
  const inactiveWireApis = wireApis.filter((wireApi) => !hasNativeProtocol(wireApi));
  const showsMessagesBridgeColumn = Boolean(messagesBinding || messagesBridgeBinding);
  const showsGeminiBridgeColumn = Boolean(geminiBridgeBinding);
  const matrixStyle = {
    "--source-route-column-count": String(
      activeWireApis.length + Number(showsMessagesBridgeColumn) + Number(showsGeminiBridgeColumn),
    ),
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
    if (!messagesBinding
      || modelIsSelected(nativeResponsesBinding, model)
      || modelIsSelected(geminiBridgeBinding, model)) return;
    const nextMessageModels = normalizedModelIds(
      [...selectedModels(messagesBinding), model],
      models,
    );
    const nextBindings = bindings.map((binding) => (
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
        reasoningMode: "disabled",
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

    if (modelIsSelected(nativeResponsesBinding, model)
      || modelIsSelected(messagesBridgeBinding, model)) return;
    if (geminiBridgeBinding) {
      onChange(bindings.map((binding) => (
        binding === geminiBridgeBinding
          ? { ...binding, modelIds: normalizedModelIds([...geminiBridgeModels, model], models) }
          : binding
      )));
      return;
    }
    onChange([
      ...bindings,
      {
        wireApi: "responses",
        adapter: "responses_to_gemini",
        reasoningMode: "disabled",
        modelIds: [model],
      },
    ]);
  };
  const addGeminiBridge = () => {
    if (geminiBridgeBinding) return;
    onChange([
      ...bindings,
      {
        wireApi: "responses",
        adapter: "responses_to_gemini",
        reasoningMode: "disabled",
        modelIds: [],
      },
    ]);
  };
  const cacheBindingLabel = (binding: SourceProtocolBinding) => (
    binding.wireApi === "messages"
      ? t("sources.cacheWriteTtlNative")
      : t("sources.cacheWriteTtlBridge")
  );
  const setCacheWriteTtl = (target: SourceProtocolBinding, cacheWriteTtl: CacheWriteTtl) => {
    onChange(bindings.map((binding) => (
      binding === target ? { ...binding, cacheWriteTtl } : binding
    )));
  };

  return (
    <section className="source-protocol-bindings" aria-labelledby={titleId}>
      <header>
        <strong id={titleId}>{t("sources.protocolsTitle")}</strong>
        <p>{t("sources.protocolsHint")}</p>
      </header>
      {inactiveWireApis.length || (!geminiBridgeBinding && wireApis.includes("responses")) ? <div className="source-route-add-formats" role="group" aria-label={t("sources.addFormats")}>
        {inactiveWireApis.map((wireApi) => {
          const { icon: Icon } = protocolPresentation[wireApi];
          const label = t(`sources.protocolCards.${wireApi}.title`);
          return <button key={wireApi} type="button" onClick={() => setNativeProtocol(wireApi, true)}>
            <Plus aria-hidden />
            <Icon aria-hidden />
            {label}
          </button>;
        })}
        {!geminiBridgeBinding && wireApis.includes("responses")
          ? <button type="button" onClick={addGeminiBridge}>
            <Plus aria-hidden />
            <Sparkles aria-hidden />
            {t("sources.addGeminiBridge")}
          </button>
          : null}
      </div> : null}
      {cacheBindings.length ? <section className="source-cache-settings" aria-label={t("sources.cacheWriteTtl")}>
        <strong>{t("sources.cacheWriteTtl")}</strong>
        <div className="source-cache-ttl-grid">
          {cacheBindings.map((binding) => {
            const label = `${t("sources.cacheWriteTtl")}: ${cacheBindingLabel(binding)}`;
            return <div key={`${binding.wireApi}:${normalizedAdapter(binding)}`} className="source-cache-ttl">
              <span>{cacheBindingLabel(binding)}</span>
              <OptionMenu className="field-option-menu" label={label} value={binding.cacheWriteTtl ?? "provider"} onChange={(value) => setCacheWriteTtl(binding, value as CacheWriteTtl)} options={[
                { value: "provider", label: t("sources.cacheWriteTtls.provider") },
                { value: "5m", label: t("sources.cacheWriteTtls.5m") },
                { value: "1h", label: t("sources.cacheWriteTtls.1h") },
              ]} />
            </div>;
          })}
        </div>
      </section> : null}
      {activeWireApis.length || showsMessagesBridgeColumn || showsGeminiBridgeColumn ? <div className="source-route-matrix" style={matrixStyle}>
        <div className="source-route-matrix-heading">
          <span>{t("sources.modelColumn")}</span>
          <div className="source-route-format-headings">
            {activeWireApis.map((wireApi) => {
              const { icon: Icon, endpoint } = protocolPresentation[wireApi];
              return (
                <label
                  key={wireApi}
                  className="source-route-format-heading selected"
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
                    checked
                    aria-label={t("sources.protocolAvailableControl", {
                      protocol: t(`sources.protocolCards.${wireApi}.title`),
                    })}
                    onChange={(event) => setNativeProtocol(wireApi, event.target.checked)}
                  />
                </label>
              );
            })}
            {showsMessagesBridgeColumn
              ? <div className="source-route-bridge-heading">
                <span className="source-route-format-icon" aria-hidden="true"><Route /></span>
                <span>
                  <strong>{t("sources.bridgeColumnTitle")}</strong>
                  <small>{t("sources.bridgeColumnHint")}</small>
                </span>
              </div>
              : null}
            {showsGeminiBridgeColumn
              ? <div className="source-route-bridge-heading">
                <span className="source-route-format-icon" aria-hidden="true"><Sparkles /></span>
                <span>
                  <strong>{t("sources.geminiBridgeColumnTitle")}</strong>
                  <small>{t("sources.geminiBridgeColumnHint")}</small>
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
                    : !messagesBridgeChecked && (directResponsesChecked || geminiBridgeChecked)
                      ? t("sources.bridgeRouteConflict")
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
                    {activeWireApis.map((wireApi) => {
                      const binding = routeBinding(wireApi, "native");
                      const checked = modelIsSelected(binding, model);
                      const assignedToOtherRoute = Boolean(binding) && bindings.some(
                        (candidate) =>
                          candidate.wireApi === wireApi
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
                      ? <label
                        className={`source-route-cell source-route-bridge-cell ${messagesBridgeChecked ? "selected" : ""}`}
                        title={messagesBridgeTitle}
                      >
                        <span className="source-route-cell-label" aria-hidden="true">
                          {t("sources.bridgeColumnTitle")}
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
                          {t("sources.geminiBridgeColumnTitle")}
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
      </div> : null}
      {showsMessagesBridgeColumn
        ? <p className="source-route-bridge-note">{t("sources.bridgeHint")}</p>
        : null}
      {showsGeminiBridgeColumn
        ? <p className="source-route-bridge-note">{t("sources.geminiBridgeHint")}</p>
        : null}
    </section>
  );
}
