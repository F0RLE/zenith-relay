import { useCallback, useMemo, useState } from "react";
import { ChevronDown, CircleDollarSign, Pause, RotateCcw, Search } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import { SourcePriceEditor } from "./SourcePriceEditor";
import { parseSourcePriceDrafts, sourcePriceDrafts, type SourcePriceDrafts } from "./sourcePriceEditorModel";
import { Button, Dialog, OptionMenu, Tabs, ToggleSwitch } from "./Ui";
import { toggle, type PoolMember } from "../poolHelpers";
import { groupModels, memberModelCatalog } from "../modelGroups";
import { useRelayState } from "../state/RelayStateProvider";
import {
  modelSelectionForMember,
  modelSelectionPayload,
} from "./poolMemberEditorModel";

export function PoolMemberEditor({ member, onClose }: { member: PoolMember; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [recoveryDelaySeconds, setRecoveryDelaySeconds] = useState(member.kind === "source" ? member.recoveryDelaySeconds ?? 0 : 0);
  const [tab, setTab] = useState("models");
  const [search, setSearch] = useState("");
  const { modelIds, enabledModels: initialEnabledModels } = modelSelectionForMember(member);
  const [enabledModels, setEnabledModels] = useState(initialEnabledModels);
  const toggleEnabledModel = useCallback((model: string) => {
    setEnabledModels((values) => toggle(values, model));
  }, []);
  const [draining, setDraining] = useState(member.draining);
  const [sourcePriceDraftsState, setSourcePriceDrafts] = useState<SourcePriceDrafts>(() => sourcePriceDrafts(member.kind === "source" ? member.modelPriceOverrides ?? {} : {}));
  const sourcePriceOverrides = useMemo(() => parseSourcePriceDrafts(sourcePriceDraftsState), [sourcePriceDraftsState]);
  const [purchaseCost, setPurchaseCost] = useState(member.kind === "account" && member.purchaseCostMicroUsd ? String(member.purchaseCostMicroUsd / 1_000_000) : "");
  const purchaseCostUsd = purchaseCost.trim() === "" ? 0 : Number(purchaseCost);
  const purchaseCostValid = Number.isFinite(purchaseCostUsd) && purchaseCostUsd >= 0 && purchaseCostUsd <= 1_000_000;
  const filteredModels = modelIds.filter((model) => model.toLowerCase().includes(search.trim().toLowerCase()));
  const catalog = memberModelCatalog(runtime?.gateway);
  const modelGroups = groupModels(filteredModels, {
    metadata: (model) => catalog.get(model.toLowerCase()),
    isNativeChatGpt: () => member.kind === "account",
  });
  const memberName = member.kind === "source" ? member.name : member.label;
  const tabs = [
    { id: "models", label: t("common.models") },
    ...(member.kind === "source" ? [{ id: "prices", label: t("sources.editorPricesTab") }] : []),
    { id: "settings", label: t("nav.settings") },
  ];
  const save = async () => {
    if (busy || !canSave || !purchaseCostValid || (member.kind === "source" && !sourcePriceOverrides)) return;
    const { allowedModels, excludedModels } = modelSelectionPayload(modelIds, enabledModels);
    const persist = () => {
      if (member.kind === "account") {
        const payload = { allowedModels, excludedModels, draining, purchaseCostMicroUsd: Math.round(purchaseCostUsd * 1_000_000) };
        return mode === "local"
          ? relayCommands.updateAccount({ accountId: member.id, ...payload })
          : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      }
      const protocolBindings = member.protocolBindings ?? [];
      const payload = { allowedModels, excludedModels, draining, priority: member.priority, weight: member.weight, recoveryDelaySeconds, modelPriceOverrides: sourcePriceOverrides ?? {}, protocolBindings };
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, ...payload };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    };
    const ok = await perform(`member-${member.id}`, persist, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog className="member-policy-dialog" title={t("pool.editMember")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave || !purchaseCostValid || (member.kind === "source" && !sourcePriceOverrides)} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={() => void save()}>{t("pool.savePolicy")}</Button></>}>
    <div className="relay-form member-editor">
      <div className="member-editor-identity"><strong data-relay-tooltip={memberName}>{memberName}</strong><span>{t(`pool.types.${member.kind}`)}</span></div>
      <Tabs value={tab} items={tabs} onChange={setTab} label={t("pool.editMember")} />
      <div role="tabpanel" aria-label={tabs.find((item) => item.id === tab)?.label}>
        {tab === "models" ? <section className="member-model-rules">
          <div className="member-model-heading"><strong>{t("pool.allowedModels")}</strong><span>{enabledModels.length} / {modelIds.length}</span></div>
          {modelIds.length > 6 ? <label className="member-model-search"><Search aria-hidden /><input type="search" aria-label={t("pool.searchMemberModels")} placeholder={t("pool.searchMemberModels")} value={search} onChange={(event) => setSearch(event.target.value)} /></label> : null}
          {modelGroups.length ? modelGroups.map((group) => <details className="source-price-group member-model-group" key={`${group.id}:${Boolean(search.trim())}`} data-model-provider={group.provider} open>
            <summary><strong>{group.provider === "other" ? t("modelGroups.other") : group.label}</strong><span>{group.items.filter((model) => enabledModels.includes(model)).length} / {group.items.length}</span><ChevronDown aria-hidden /></summary>
            <ul>{group.items.map((model) => {
              const enabled = enabledModels.includes(model);
              return <li key={model} data-member-model-id={model} data-enabled={String(enabled)}>
                <label><code>{model}</code><ToggleSwitch className="member-model-switch" role="switch" label={t("pool.allowMemberModel", { model })} checked={enabled} onChange={() => toggleEnabledModel(model)} /></label>
              </li>;
            })}</ul>
          </details>) : <p className="form-note">{t(modelIds.length ? "common.noResults" : "models.emptyDescription")}</p>}
        </section> : null}
        {tab === "prices" && member.kind === "source" ? <SourcePriceEditor source={member} drafts={sourcePriceDraftsState} onChange={setSourcePriceDrafts} presentation="member" /> : null}
        {tab === "settings" ? <div className="member-editor-settings">
          {member.kind === "source" ? <div className="member-editor-setting" data-member-setting="recovery"><span className="member-setting-label"><RotateCcw aria-hidden /><span>{t("sources.recoveryDelay")}</span></span><OptionMenu className="field-option-menu" label={t("sources.recoveryDelay")} value={String(recoveryDelaySeconds)} onChange={(value) => setRecoveryDelaySeconds(Number(value))} options={[0, 5, 30, 60, 300, 900].map((seconds) => ({ value: String(seconds), label: seconds === 0 ? t("sources.recoveryAutomatic") : formatRecoveryDelay(seconds, t) }))} /></div> : <>
            <label className="member-editor-setting" data-member-setting="drain"><span className="member-setting-label"><Pause aria-hidden /><span>{t("accounts.drain")}</span></span><ToggleSwitch className="member-model-switch" role="switch" label={t("accounts.drain")} checked={draining} onChange={setDraining} /></label>
            <label className="member-editor-setting" data-member-setting="cost"><span className="member-setting-label"><CircleDollarSign aria-hidden /><span>{t("accounts.accountValue.purchaseCost")}</span></span><input type="number" min="0" max="1000000" step="0.01" aria-invalid={!purchaseCostValid || undefined} value={purchaseCost} onChange={(event) => setPurchaseCost(event.target.value)} placeholder={t("pool.purchaseCostNotSet")} /></label>
          </>}
        </div> : null}
      </div>
    </div>
  </Dialog>;
}

function formatRecoveryDelay(seconds: number, t: TFunction) {
  return seconds < 60 ? t("sources.recoverySeconds", { count: seconds }) : t("sources.recoveryMinutes", { count: seconds / 60 });
}
