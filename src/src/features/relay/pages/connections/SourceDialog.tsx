import { useMemo, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { SourceSummary } from "../../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue } from "../../components/ApiProviderForm";
import { SourceProtocolRoutingDisclosure } from "../../components/SourceProtocolRoutingDisclosure";
import { SecretField, Button, Dialog } from "../../components/Ui";
import { SourcePriceEditor, parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "../../components/SourcePriceEditor";
import { effectiveSourceProtocolBindings } from "../../sourceProtocolBindings";
import { useRelayState } from "../../state/RelayStateProvider";
export function SourceDialog({ source, onClose, addToPool = false }: { source: SourceSummary | null; onClose: () => void; addToPool?: boolean }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [protocolBindings, setProtocolBindings] = useState(() => source ? effectiveSourceProtocolBindings(source) : []);
  const [priceDrafts, setPriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(source?.modelPriceOverrides ?? {}));
  const modelPriceOverrides = useMemo(() => parseSourcePriceDrafts(priceDrafts), [priceDrafts]);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (source && !modelPriceOverrides) return;
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
      const wireApi = protocolBindings[0]?.wireApi ?? source.wireApi;
      const update = { name, baseUrl, wireApi, protocolBindings, models: source.models, allowedModels: source.allowedModels, excludedModels: source.excludedModels, draining: source.draining, priority: source.priority, weight: source.weight, recoveryDelaySeconds: source.recoveryDelaySeconds, modelPriceOverrides };
      if (mode !== "remote") {
        await relayCommands.updateSource({ sourceId: source.id, ...update });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...update, ...(apiKey ? { apiKey } : {}) });
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded");
    if (ok) onClose();
  };
  const canShowSave = Boolean(source || provider.kind);
  const dialogClassName = source
    ? "source-edit-dialog"
    : `source-add-dialog ${canShowSave ? "source-add-configuring" : "source-add-selecting"}`;
  const footer = canShowSave
    ? <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={(!source && !apiProviderReady(provider)) || (Boolean(source) && !protocolBindings.length) || !modelPriceOverrides} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>
    : <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>;
  return <Dialog wide className={dialogClassName} title={source ? t("sources.edit") : addToPool ? t("sources.addToPool") : t("sources.add")} onClose={onClose} footer={footer}><form id="source-form" className="relay-form source-form" onSubmit={submit}>{source ? <><section className="source-form-section"><header><h3>{t("sources.connection")}</h3></header><div className="source-identity-grid"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label></div><div className="source-access-grid"><SecretField label={t("sources.replaceKey")} value={apiKey} onChange={setApiKey} /></div></section><SourceProtocolRoutingDisclosure models={source.models} value={protocolBindings} onChange={setProtocolBindings} /><SourcePriceEditor source={source} drafts={priceDrafts} onChange={setPriceDrafts} /></> : <ApiProviderForm value={provider} onChange={setProvider} />}</form></Dialog>;
}
