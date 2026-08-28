import { useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { SourceSummary } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import { SourceProtocolRoutingDisclosure } from "../../components/SourceProtocolRoutingDisclosure";
import { SecretField, Button, Dialog, ErrorDetailsDialog, Tabs } from "../../components/Ui";
import { SourcePriceEditor } from "../../components/SourcePriceEditor";
import { parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "../../components/sourcePriceEditorModel";
import { effectiveSourceProtocolBindings, normalizedBindings } from "../../sourceProtocolBindings";
import { useRelayState } from "../../state/RelayStateProvider";
import type { FeedbackError } from "../../state/feedback";

type SourceEditTab = "main" | "routes" | "adapters" | "prices";
type SourceAddStep = "provider" | "configure";

export function SourceDialog({ source, onClose, addToPool = false }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [protocolBindings, setProtocolBindings] = useState(() => source ? effectiveSourceProtocolBindings(source) : []);
  const [priceDrafts, setPriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(source?.modelPriceOverrides ?? {}));
  const [activeTab, setActiveTab] = useState<SourceEditTab>("main");
  const [addStep, setAddStep] = useState<SourceAddStep>("provider");
  const [operationError, setOperationError] = useState<{ error: FeedbackError; messageKey: string } | null>(null);
  const modelPriceOverrides = useMemo(() => parseSourcePriceDrafts(priceDrafts), [priceDrafts]);
  const sourceEditTabs = [
    { id: "main", label: t("sources.editorMainTab") },
    { id: "routes", label: t("sources.editorRoutesTab") },
    { id: "adapters", label: t("sources.editorAdaptersTab") },
    { id: "prices", label: t("sources.editorPricesTab") },
  ];

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (source && !modelPriceOverrides) return;
    setOperationError(null);
    const ok = await perform("source-save", async () => {
      if (!source) {
        const payload = apiProviderSourceInput(provider);
        const created = mode !== "remote"
          ? await relayCommands.createSource(payload) as { id: string }
          : await relayCommands.remoteAction({ type: "create_source" }, payload) as { id: string };
        if (addToPool) {
          if (mode !== "remote") await relayCommands.setPoolMembership([], [created.id], true);
          else await relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [created.id], inPool: true });
        }
        return;
      }
      const normalizedProtocolBindings = normalizedBindings(protocolBindings, source.models);
      const wireApi = normalizedProtocolBindings[0]?.wireApi ?? source.wireApi;
      const update = { name, baseUrl, wireApi, protocolBindings: normalizedProtocolBindings, models: source.models, allowedModels: source.allowedModels, excludedModels: source.excludedModels, draining: source.draining, priority: source.priority, weight: source.weight, recoveryDelaySeconds: source.recoveryDelaySeconds, modelPriceOverrides };
      if (mode !== "remote") {
        await relayCommands.updateSource({ sourceId: source.id, ...update });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...update, ...(apiKey ? { apiKey } : {}) });
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded", {
      reportError: false,
      onError: (error, messageKey) => setOperationError({ error, messageKey }),
    });
    if (ok) onClose();
  };
  const selectProvider = (nextProvider: typeof provider) => {
    setProvider(nextProvider);
    if (!nextProvider.kind) setAddStep("provider");
    else if (!provider.kind) setAddStep("configure");
  };
  const backToProviderStep = () => {
    setProvider({
      ...defaultApiProviderValue(),
      apiKey: provider.apiKey,
      models: provider.models ?? [],
      modelCatalogMode: provider.modelCatalogMode ?? "automatic",
      autoAssignModels: provider.autoAssignModels !== false,
    });
    setAddStep("provider");
  };
  const configuredAdapterLabel = provider.protocolBindings[0]
    ? t(`sources.protocolCards.${provider.protocolBindings[0].wireApi}.title`)
    : t("sources.routingPending");
  const adapterSummary = provider.modelCatalogMode === "manual"
    ? t("sources.manualModelsCount", { count: provider.models?.length ?? 0 })
    : t("sources.modelsAutomatic");
  const dialogClassName = source
    ? "source-edit-dialog"
    : `source-add-dialog source-add-${addStep}`;
  const footer = source
    ? <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!protocolBindings.length || !modelPriceOverrides} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>
    : addStep === "provider"
      ? <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" disabled={!provider.kind} onClick={() => setAddStep("configure")}>{t("common.continue")}</Button></>
      : <><Button variant="secondary" onClick={backToProviderStep}>{t("common.back")}</Button><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!apiProviderReady(provider)} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>;
  return <><Dialog wide className={dialogClassName} title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={footer}><form id="source-form" className="relay-form source-form" onSubmit={submit}>{source ? <><Tabs value={activeTab} items={sourceEditTabs} onChange={(tab) => setActiveTab(tab as SourceEditTab)} label={t("sources.editorTabsLabel")} />
     {activeTab === "main" ? <section className="source-editor-tab-panel source-editor-main" role="tabpanel" aria-label={t("sources.editorMainTab")}><section className="source-form-section source-basic-fields"><div className="source-identity-grid"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label></div><div className="source-access-grid"><SecretField label={t("sources.replaceKey")} value={apiKey} onChange={setApiKey} /></div></section></section> : null}
    {activeTab === "routes" ? <section className="source-editor-tab-panel" role="tabpanel" aria-label={t("sources.editorRoutesTab")}><SourceProtocolRoutingDisclosure models={source.models} value={protocolBindings} onChange={setProtocolBindings} routeGroup="native" /></section> : null}
    {activeTab === "adapters" ? <section className="source-editor-tab-panel" role="tabpanel" aria-label={t("sources.editorAdaptersTab")}><SourceProtocolRoutingDisclosure models={source.models} value={protocolBindings} onChange={setProtocolBindings} routeGroup="adapters" /></section> : null}
    {activeTab === "prices" ? <section className="source-editor-tab-panel" role="tabpanel" aria-label={t("sources.editorPricesTab")}><SourcePriceEditor source={source} drafts={priceDrafts} onChange={setPriceDrafts} presentation="tab" /></section> : null}
  </> : <>
    <ol className="source-add-steps" aria-label={t("sources.addFlowSteps")}>
      <li className={addStep === "provider" ? "active" : "complete"}><span>1</span>{t("sources.addFlowStepProvider")}</li>
      <li className={addStep === "configure" ? "active" : ""}><span>2</span>{t("sources.addFlowStepConnection")}</li>
    </ol>
    <section className="source-add-flow-heading">
      <div>
        <h3>{addStep === "provider" ? t("sources.addFlowProviderTitle") : t("sources.addFlowConnectionTitle")}</h3>
      </div>
    </section>
    <ApiProviderForm
      value={provider}
      onChange={selectProvider}
      allowManualModels={mode !== "remote"}
      showConfiguration={addStep === "configure"}
      showRouting={false}
      showIntro={false}
      showSelectionSummary={false}
    />
    {addStep === "configure" ? <details className="source-add-adapters">
      <summary>
        <span className="source-add-adapters-copy"><strong>{t("sources.configureAdapters")}</strong><small>{t("sources.configureAdaptersHint")}</small></span>
        <span className="source-add-adapters-state"><b>{configuredAdapterLabel}</b><small>{adapterSummary}</small></span>
      </summary>
      <SourceProtocolRoutingDisclosure
        models={provider.modelCatalogMode === "manual" ? (provider.models ?? []) : []}
        value={provider.protocolBindings}
        showSimplePicker={mode !== "remote"}
        autoAssignModels={provider.autoAssignModels !== false}
        exclusiveSimplePicker
        onChange={(protocolBindings) => selectProvider({
          ...provider,
          protocolBindings,
          wireApi: protocolBindings[0]?.wireApi ?? provider.wireApi,
        })}
      />
    </details> : null}
  </>}</form></Dialog>{operationError ? <ErrorDetailsDialog error={operationError.error} message={t(operationError.messageKey)} onClose={() => setOperationError(null)} /> : null}</>;
}
