import { useMemo, useRef, useState, type FormEvent } from "react";
import { Link2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { RelayMode, SourceSummary } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import { SecretField, Button, Dialog, ErrorDetailsDialog, OptionMenu, Tabs } from "../../components/Ui";
import { SourcePriceEditor } from "../../components/SourcePriceEditor";
import { parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "../../components/sourcePriceEditorModel";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import type { FeedbackError } from "../../state/feedback";
import { selectApiProvider } from "../../components/apiProviderModel";
import { useProxyPool } from "../../hooks/useProxyPool";

type SourceEditTab = "main" | "prices";

function SourceProxyField({ proxyId, onChange }: { proxyId: string | null; onChange: (proxyId: string | null) => void }) {
  const { t } = useTranslation();
  const { pool } = useProxyPool();
  const entries = pool?.entries ?? [];
  if (!pool || (!entries.length && !proxyId)) return null;
  const options = [
    { value: "", label: t("sources.proxyNone") },
    ...(proxyId != null && !entries.some((entry) => entry.id === proxyId) ? [{ value: proxyId, label: t("sources.proxyMissing") }] : []),
    ...entries.map((entry) => ({ value: entry.id, label: entry.endpoint })),
  ];
  return (
    <div className="relay-field source-proxy-field">
      <span>{t("sources.proxy")}</span>
      <OptionMenu
        className="field-option-menu"
        label={t("sources.proxy")}
        value={proxyId ?? ""}
        align="start"
        options={options}
        onChange={(value) => onChange(value || null)}
      />
      <small>{t("sources.proxyHint")}</small>
    </div>
  );
}

export function SourceDialog({ source: initialSource, onClose, addToPool = false, modeOverride, onCreated }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean; modeOverride?: RelayMode; onCreated?: () => void }) {
  const { t } = useTranslation();
  const { mode: currentMode, runtime, perform, busy } = useRelayState();
  const relayMode = modeOverride ?? currentMode;
  const [savedSource, setSavedSource] = useState(initialSource);
  const createdSourceId = useRef<string | null>(null);
  const source = relayMode === currentMode ? runtime?.sources.find((sourceOption) => sourceOption.id === savedSource?.id) ?? savedSource : savedSource;
  const [provider, setProvider] = useState(() => selectApiProvider(defaultApiProviderValue(), "custom"));
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [proxyId, setProxyId] = useState<string | null>(source?.proxyId ?? null);
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
          const sourceInput = apiProviderSourceInput(provider);
          const proxyInput = relayMode !== "remote" && proxyId ? { proxyId } : {};
          const created = relayMode !== "remote"
            ? await relayCommands.createSource({ ...sourceInput, ...proxyInput }) as { id: string }
            : await relayCommands.remoteAction({ type: "create_source" }, sourceInput) as { id: string };
          // Creation has committed even if the following snapshot or membership
          // operation fails. A retry must continue with this source's identity.
          createdSourceId.current = created.id;
        }
        const latestSource = (relayMode !== "remote" ? await relayCommands.localState() : await relayCommands.remoteState())?.sources.find((sourceOption) => sourceOption.id === createdSourceId.current);
        if (latestSource) {
          setSavedSource(latestSource);
          setName(latestSource.name);
          setBaseUrl(latestSource.baseUrl);
          setProxyId(latestSource.proxyId ?? null);
          setPriceDrafts(sourcePriceDrafts(latestSource.modelPriceOverrides ?? {}));
          setProvider(defaultApiProviderValue());
        }
        // Membership depends on the saved source, not generation evidence.
        if (addToPool && !latestSource?.inPool) {
          await updatePoolMembership(relayMode, { accountIds: [], sourceIds: [createdSourceId.current], inPool: true });
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
      if (relayMode !== "remote") {
        await relayCommands.updateSource({ sourceId: source.id, ...update, proxyId });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...update, ...(apiKey ? { apiKey } : {}) });
      }
      if (addToPool && !initialSource && !source.inPool) {
        const latestSource = (relayMode !== "remote" ? await relayCommands.localState() : await relayCommands.remoteState())?.sources.find((sourceOption) => sourceOption.id === source.id);
        if (latestSource && !latestSource.inPool) {
          await updatePoolMembership(relayMode, { accountIds: [], sourceIds: [source.id], inPool: true });
        }
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded", {
      backgroundRefresh: true,
      reportError: false,
      onError: (error, messageKey) => setOperationError({ error, messageKey }),
    });
    if (ok) {
      onCreated?.();
      onClose();
    }
  };
  const dialogClassName = source ? "source-edit-dialog connection-dialog" : "source-add-dialog";
  const submitSourceForm = () => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit();
  const footer = (
    <>
      <Button
        variant="primary"
        busy={busy === "source-save"}
        disabled={source ? !modelPriceOverrides : !apiProviderReady(provider)}
        onClick={submitSourceForm}
      >
        {t(source ? "common.save" : "sources.add")}
      </Button>
    </>
  );
  return (
    <>
      <Dialog
        wide
        className={dialogClassName}
        title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")}
        onClose={onClose}
        footer={source || provider.kind ? footer : undefined}
      >
        <form id="source-form" className="relay-form source-form" onSubmit={submit}>
          {source ? (
            <>
              <div className="connection-dialog-context source-editor-context">
                <div className="source-editor-icon"><Link2 aria-hidden /></div>
                <div className="source-editor-identity">
                  <strong>{source.name}</strong>
                  <small>{source.baseUrl}</small>
                </div>
                <span>{t("sources.groupModelsCount", { count: source.models.length })}</span>
              </div>
              <Tabs value={activeTab} items={sourceEditTabs} onChange={(tab) => setActiveTab(tab as SourceEditTab)} label={t("sources.editorTabsLabel")} />
              {activeTab === "main" ? (
                <section className="source-editor-tab-panel source-editor-main" role="tabpanel" aria-label={t("sources.editorMainTab")}>
                  <section className="source-form-section source-basic-fields">
                    <div className="source-identity-grid">
                      <label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label>
                      <label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label>
                    </div>
                    <div className="source-access-grid">
                      <SecretField label={t("sources.replaceKey")} value={apiKey} onChange={setApiKey} />
                      <p className="form-note">{t("sources.keepKeyHint")}</p>
                    </div>
                    {relayMode !== "remote" ? <SourceProxyField proxyId={proxyId} onChange={setProxyId} /> : null}
                  </section>
                </section>
              ) : null}
              {activeTab === "prices" ? (
                <section className="source-editor-tab-panel" role="tabpanel" aria-label={t("sources.editorPricesTab")}>
                  <SourcePriceEditor source={source} drafts={priceDrafts} onChange={setPriceDrafts} presentation="tab" />
                </section>
              ) : null}
            </>
          ) : (
            <>
              <ApiProviderForm value={provider} onChange={setProvider} />
              {provider.kind && relayMode !== "remote" ? <SourceProxyField proxyId={proxyId} onChange={setProxyId} /> : null}
            </>
          )}
        </form>
      </Dialog>
      {operationError ? (
        <ErrorDetailsDialog error={operationError.error} message={t(operationError.messageKey)} onClose={() => setOperationError(null)} />
      ) : null}
    </>
  );
}
