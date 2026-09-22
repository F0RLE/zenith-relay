import { useMemo, useRef, useState, type FormEvent } from "react";
import { Link2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { SourceSummary } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import { SecretField, Button, Dialog, ErrorDetailsDialog, Tabs } from "../../components/Ui";
import { SourcePriceEditor } from "../../components/SourcePriceEditor";
import { parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "../../components/sourcePriceEditorModel";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import type { FeedbackError } from "../../state/feedback";

type SourceEditTab = "main" | "prices";
export function SourceDialog({ source: initialSource, onClose, addToPool = false }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [savedSource, setSavedSource] = useState(initialSource);
  const createdSourceId = useRef<string | null>(null);
  const source = runtime?.sources.find((value) => value.id === savedSource?.id) ?? savedSource;
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [priceDrafts, setPriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(source?.modelPriceOverrides ?? {}));
  const [activeTab, setActiveTab] = useState<SourceEditTab>("main");
  const [operationError, setOperationError] = useState<{ error: FeedbackError; messageKey: string } | null>(null);
  const modelPriceOverrides = useMemo(() => parseSourcePriceDrafts(priceDrafts), [priceDrafts]);
  const sourceEditTabs = [
    { id: "main", label: t("sources.editorMainTab") },
    { id: "prices", label: t("sources.editorPricesTab") },
  ];

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (source && !modelPriceOverrides) return;
    setOperationError(null);
    const ok = await perform("source-save", async () => {
      if (!source) {
        if (!createdSourceId.current) {
          const payload = apiProviderSourceInput(provider);
          const created = mode !== "remote"
            ? await relayCommands.createSource(payload) as { id: string }
            : await relayCommands.remoteAction({ type: "create_source" }, payload) as { id: string };
          // Creation has committed even if the following snapshot or membership
          // operation fails. A retry must continue with this source's identity.
          createdSourceId.current = created.id;
        }
        const latest = (mode !== "remote" ? await relayCommands.localState() : await relayCommands.remoteState())?.sources.find((value) => value.id === createdSourceId.current);
        if (latest) {
          setSavedSource(latest);
          setName(latest.name);
          setBaseUrl(latest.baseUrl);
          setPriceDrafts(sourcePriceDrafts(latest.modelPriceOverrides ?? {}));
          setProvider(defaultApiProviderValue());
        }
        // Membership depends on the saved source, not generation evidence.
        if (addToPool && !latest?.inPool) {
          await updatePoolMembership(mode, { accountIds: [], sourceIds: [createdSourceId.current], inPool: true });
        }
        return;
      }
      const update = {
        name,
        baseUrl,
        pricingProvider: source.pricingProvider ?? null,
        officialProviderFamily: source.officialProviderFamily ?? null,
        wireApi: source.wireApi,
        protocolBindings: source.protocolBindings ?? [],
        models: source.models,
        allowedModels: source.allowedModels,
        excludedModels: source.excludedModels,
        draining: source.draining,
        priority: source.priority,
        weight: source.weight,
        recoveryDelaySeconds: source.recoveryDelaySeconds,
        modelPriceOverrides,
      };
      if (mode !== "remote") {
        await relayCommands.updateSource({ sourceId: source.id, ...update });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...update, ...(apiKey ? { apiKey } : {}) });
      }
      if (addToPool && !initialSource && !source.inPool) {
        const latest = (mode !== "remote" ? await relayCommands.localState() : await relayCommands.remoteState())?.sources.find((value) => value.id === source.id);
        if (latest && !latest.inPool) {
          await updatePoolMembership(mode, { accountIds: [], sourceIds: [source.id], inPool: true });
        }
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded", {
      reportError: false,
      onError: (error, messageKey) => setOperationError({ error, messageKey }),
    });
    if (ok && source) onClose();
  };
  const dialogClassName = source ? `source-edit-dialog connection-dialog${activeTab === "prices" ? " source-prices-dialog" : ""}` : "source-add-dialog";
  const footer = source
    ? <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!modelPriceOverrides} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>
    : <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!apiProviderReady(provider)} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>;
  return <><Dialog wide className={dialogClassName} title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={footer}><form id="source-form" className="relay-form source-form" onSubmit={submit}>{source ? <><div className="connection-dialog-context"><Link2 aria-hidden /><strong>{source.name}</strong><span>{t("sources.groupModelsCount", { count: source.models.length })}</span></div><Tabs value={activeTab} items={sourceEditTabs} onChange={(tab) => setActiveTab(tab as SourceEditTab)} label={t("sources.editorTabsLabel")} />
    {activeTab === "main" ? <section className="source-editor-tab-panel source-editor-main" role="tabpanel" aria-label={t("sources.editorMainTab")}>
      <section className="source-form-section source-basic-fields"><div className="source-identity-grid">
        <label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label>
        <label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label>
      </div><div className="source-access-grid"><SecretField label={t("sources.replaceKey")} value={apiKey} onChange={setApiKey} /><p className="form-note">{t("sources.keepKeyHint")}</p></div></section>
    </section> : null}
    {activeTab === "prices" ? <section className="source-editor-tab-panel" role="tabpanel" aria-label={t("sources.editorPricesTab")}><SourcePriceEditor source={source} drafts={priceDrafts} onChange={setPriceDrafts} presentation="tab" /></section> : null}
  </> : <>
    <ApiProviderForm value={provider} onChange={setProvider} />
  </>}</form></Dialog>{operationError ? <ErrorDetailsDialog error={operationError.error} message={t(operationError.messageKey)} onClose={() => setOperationError(null)} /> : null}</>;
}
