import { useId } from "react";
import { useTranslation } from "react-i18next";
import type { SourceProtocolBinding, SourceSummary, SourceWireApi } from "../api/types";

export const sourceWireApis: SourceWireApi[] = [
  "responses",
  "messages",
  "chat_completions",
];

function isSourceWireApi(value: string): value is SourceWireApi {
  return sourceWireApis.includes(value as SourceWireApi);
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
  const seen = new Set<SourceWireApi>();
  return bindings.flatMap((binding) => {
    if (!isSourceWireApi(binding.wireApi) || seen.has(binding.wireApi)) return [];
    seen.add(binding.wireApi);
    const modelIds = binding.modelIds.length
      ? normalizedModelIds(binding.modelIds, availableModels)
      : [];
    return [{ wireApi: binding.wireApi, modelIds }];
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

export function SourceProtocolBindingsSummary({
  source,
}: {
  source: Pick<SourceSummary, "wireApi" | "protocolBindings" | "models">;
}) {
  const { t } = useTranslation();
  return (
    <span className="source-protocol-summary">
      {effectiveSourceProtocolBindings(source)
        .map((binding) => t(`sources.protocols.${binding.wireApi}`))
        .join(", ")}
    </span>
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
  const selectedBinding = (wireApi: SourceWireApi) =>
    bindings.find((binding) => binding.wireApi === wireApi);
  const selectedModels = (binding: SourceProtocolBinding) =>
    binding.modelIds.length ? binding.modelIds : models;

  const setProtocol = (wireApi: SourceWireApi, selected: boolean) => {
    if (!selected) {
      onChange(bindings.filter((binding) => binding.wireApi !== wireApi));
      return;
    }
    onChange([
      ...bindings,
      {
        wireApi,
        // Before discovery a blank list means "use discovered models". Once
        // models exist we make the all-models choice explicit, so unchecking
        // a model can never accidentally expand the binding again.
        modelIds: [...models],
      },
    ]);
  };

  const setModel = (
    wireApi: SourceWireApi,
    model: string,
    selected: boolean,
  ) => {
    onChange(bindings.map((binding) => {
      if (binding.wireApi !== wireApi) return binding;
      const selectedIds = selectedModels(binding);
      const modelKey = model.toLowerCase();
      const nextModelIds = selected
        ? [...selectedIds, model]
        : selectedIds.filter((candidate) => candidate.toLowerCase() !== modelKey);
      // A selected protocol with an empty model list is interpreted by the
      // persisted contract as "all discovered models". Keep one selected
      // model instead; turning a protocol off is the explicit way to remove
      // its entire binding.
      return {
        ...binding,
        modelIds: nextModelIds.length ? normalizedModelIds(nextModelIds, models) : selectedIds,
      };
    }));
  };

  return (
    <section className="source-protocol-bindings" aria-labelledby={titleId}>
      <header>
        <strong id={titleId}>{t("sources.protocolsTitle")}</strong>
        <p>{t("sources.protocolsHint")}</p>
      </header>
      <div className="client-protocol-grid">
        {wireApis.map((wireApi) => {
          const binding = selectedBinding(wireApi);
          return (
            <label key={wireApi} className={binding ? "selected" : ""}>
              <input
                type="checkbox"
                checked={Boolean(binding)}
                onChange={(event) => setProtocol(wireApi, event.target.checked)}
              />
              <span>
                <strong>{t(`sources.protocols.${wireApi}`)}</strong>
                <small>{t(`sources.protocolHints.${wireApi}`)}</small>
              </span>
            </label>
          );
        })}
      </div>
      {models.length
        ? bindings.map((binding) => {
          const selected = selectedModels(binding);
          return (
            <fieldset key={binding.wireApi} className="source-protocol-models">
              <legend>{t("sources.protocolModels", {
                protocol: t(`sources.protocols.${binding.wireApi}`),
              })}</legend>
              <p>{t("sources.protocolModelsHint")}</p>
              <div className="scope-grid">
                {models.map((model) => {
                  const checked = selected.some(
                    (candidate) => candidate.toLowerCase() === model.toLowerCase(),
                  );
                  const lastSelectedModel = checked && selected.length === 1;
                  return (
                    <label key={model}>
                      <input
                        type="checkbox"
                        checked={checked}
                        disabled={lastSelectedModel}
                        onChange={(event) => setModel(binding.wireApi, model, event.target.checked)}
                      />
                      <span>{model}</span>
                    </label>
                  );
                })}
              </div>
            </fieldset>
          );
        })
        : null}
    </section>
  );
}
