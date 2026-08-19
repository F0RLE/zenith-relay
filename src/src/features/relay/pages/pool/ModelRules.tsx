import { useState } from "react";
import { BrainCircuit, Loader2, Pencil, Power, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ModelSummary } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, StatusIcon } from "../../components/Ui";
import { supportsCacheWritePricing } from "../../modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../../modelPricing";
import { groupModelSummariesForLauncher, modelSummaries, toggle } from "../../poolHelpers";
import {
  formatModelPrice,
  formatReasoningEffort,
} from "../../poolFormatting";
import { useRelayState } from "../../state/RelayStateProvider";

export function ModelRulesView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [priceModel, setPriceModel] = useState<ModelSummary | null>(null);
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  const models = runtime ? modelSummaries(runtime) : [];
  const modelGroups = groupModelSummariesForLauncher(models, runtime?.accounts ?? []);
  const canEditPrice = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("model_pricing"));
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  if (!models.length) return <EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} />;
  return <><section className="model-rules relay-compact-content">
    <header>
      <div className="model-rules-copy"><h2>{t("models.visible")}</h2><p>{t("models.explanation")}</p></div>
    </header>
    <div className="relay-table-wrap"><table className="relay-table model-rules-table">
      <colgroup><col data-column="model" /><col data-column="codex" /><col data-column="price" /><col data-column="members" /><col data-column="actions" /></colgroup>
      <thead><tr><th>{t("common.model")}</th><th>{t("models.codexColumn")}</th><th>{t("models.priceColumn")}</th><th>{t("pool.members")}</th><th>{t("common.actions")}</th></tr></thead>
      {modelGroups.map((group) => <tbody key={group.id}>
      <tr className="model-group-row"><th colSpan={5} scope="rowgroup"><strong>{t(`modelGroups.${group.id}`, { defaultValue: group.label })}</strong><span>{t("models.groupCount", { count: group.items.length })}</span></th></tr>
      {group.items.map((model) => {
      const toggling = busy === `model-toggle-${model.id}`;
      const hasPrice = model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
      const cachedInputPrice = model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion;
      const priceParts = hasPrice ? [
        { label: t("models.inputPriceLabel"), value: formatModelPrice(model.inputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.outputPriceLabel"), value: formatModelPrice(model.outputMicroUsdPerMillion!, i18n.language) },
        { label: t("models.cachedInputPriceLabel"), value: formatModelPrice(cachedInputPrice!, i18n.language) },
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite5mMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite5mPriceLabel"), value: formatModelPrice(model.cacheWrite5mMicroUsdPerMillion, i18n.language) }] : []),
        ...(supportsCacheWritePricing(model.id) && model.cacheWrite1hMicroUsdPerMillion != null ? [{ label: t("models.cacheWrite1hPriceLabel"), value: formatModelPrice(model.cacheWrite1hMicroUsdPerMillion, i18n.language) }] : []),
      ] : [];
      const displayName = model.codexDisplayName || model.id;
      const toggleLabel = t(model.enabled ? "models.disable" : "models.enable", { model: model.id });
      const hasReasoningModes = (model.reasoningLevels?.length ?? 0) > 0 || (model.reasoningSupportedLevels?.length ?? 0) > 0;
      const canEditReasoning = Boolean(model.reasoningConfigurable && hasReasoningModes);
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"}>
        <td data-column="model"><div className="model-rule-identity"><strong title={displayName}>{displayName}</strong>{displayName !== model.id ? <code title={model.id}>{model.id}</code> : null}<span className={`model-rule-state ${model.enabled ? "ready" : "disabled"}`}><StatusIcon status={model.enabled ? "ready" : "disabled"} label={t(model.enabled ? "models.available" : "models.disabled")} /><span>{t(model.enabled ? "models.available" : "models.disabled")}</span></span></div></td>
        <td data-column="codex"><div className={`model-codex-state ${model.codexVisible ? "visible" : "hidden"}`}><BrainCircuit aria-hidden /><span><strong>{t(model.codexVisible ? "models.codexVisible" : model.enabled ? "models.codexUnsupported" : "models.codexDisabled")}</strong></span></div></td>
        <td data-column="price"><div className="model-price">{hasPrice ? <>{priceParts.map((part) => <span className="model-price-value" key={part.label}><small>{part.label}</small><strong>{part.value}</strong></span>)}{model.customPrice ? <small className="model-price-note custom">{t("models.customPrice")}</small> : null}</> : <span className="model-price-empty muted">{t("models.priceUnavailable")}</span>}</div></td>
        <td data-column="members"><span className="model-members">{t("pool.membersCount", { count: model.memberCount })}</span></td>
        <td data-column="actions"><div className="model-rule-actions">{canEditPrice ? <IconButton data-model-price-edit={model.id} label={t("models.editPrice", { model: model.id })} icon={<Pencil aria-hidden />} onClick={() => setPriceModel(model)} /> : null}{hasReasoningModes || model.reasoningProbe ? <IconButton data-model-reasoning-edit={model.id} label={t(canEditReasoning ? "models.editReasoning" : "models.viewReasoning", { model: model.id })} icon={<BrainCircuit aria-hidden />} onClick={() => setReasoningModel(model)} /> : null}<IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
    })}</tbody>)}
    </table></div>
  </section>{priceModel ? <ModelPriceDialog key={priceModel.id} model={priceModel} onClose={() => setPriceModel(null)} /> : null}{reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}</>;
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const detectedLevels = model.reasoningLevels ?? [];
  const supportedLevels = model.reasoningSupportedLevels ?? [];
  const levels = [...new Set([...supportedLevels, ...detectedLevels])];
  const editable = Boolean(model.reasoningConfigurable && levels.length);
  const [allowedLevels, setAllowedLevels] = useState(model.reasoningAllowedLevels ?? []);
  const operation = `model-reasoning-${model.id}`;
  const probe = model.reasoningProbe;
  const probeProgress = probe ? t("models.reasoningProbeProgress", probe) : "";
  const save = async () => {
    const ok = await perform(operation, () => mode === "local"
      ? relayCommands.setModelReasoning(model.id, allowedLevels)
      : relayCommands.remoteAction({ type: "set_model_reasoning" }, { modelId: model.id, allowedLevels }), "feedback.saved");
    if (ok) onClose();
  };
  const automatic = allowedLevels.length === 0;
  const label = (level: string) => t(`usage.reasoningEfforts.${level}`, { defaultValue: formatReasoningEffort(level) });
  const toggleAllowedLevel = (level: string) => setAllowedLevels((current) => {
    if (current.length) return toggle(current, level);
    return detectedLevels.includes(level) ? [level] : [...detectedLevels, level];
  });
  return <Dialog title={t("models.reasoningTitle")} onClose={onClose} footer={editable ? <><Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} onClick={() => void save()}>{t("common.save")}</Button></> : <Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <div className="model-reasoning-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t(editable ? "models.reasoningHint" : "models.reasoningReadOnlyHint")}</span></div>
      {probe ? <div className="model-reasoning-detected"><span>{t("models.reasoningProbeStatus")}</span><strong>{t(`models.reasoningProbeStatuses.${probe.status}`, { defaultValue: probe.status })} · {probeProgress}{probe.lastProbeAt ? ` · ${t("models.reasoningProbeLastAt", { value: new Date(probe.lastProbeAt).toLocaleString(i18n.language) })}` : ""}</strong></div> : null}
      <div className="model-reasoning-detected"><span>{t("models.reasoningAvailable")}</span><strong>{supportedLevels.length ? supportedLevels.map(label).join(", ") : t("models.reasoningDetectedEmpty")}</strong></div>
      {levels.length && editable ? <>
        <div className="model-reasoning-detected"><span>{t("models.reasoningDetected")}</span><strong>{detectedLevels.length ? detectedLevels.map(label).join(", ") : t("models.reasoningDetectedEmpty")}</strong></div>
        <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningTitle")}>
          <button type="button" aria-pressed={automatic} className={automatic ? "selected" : undefined} onClick={() => setAllowedLevels([])}>{t("models.reasoningAuto")}</button>
          {levels.map((level) => <button key={level} type="button" role="checkbox" aria-checked={!automatic && allowedLevels.includes(level)} className={!automatic && allowedLevels.includes(level) ? "selected" : undefined} onClick={() => toggleAllowedLevel(level)}>{label(level)}</button>)}
        </div>
      </> : levels.length ? <div className="model-reasoning-options" role="list" aria-label={t("models.reasoningTitle")}>
        {levels.map((level) => <span key={level} className="model-reasoning-mode" role="listitem">{label(level)}</span>)}
      </div> : null}
    </div>
  </Dialog>;
}

function ModelPriceDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [inputPrice, setInputPrice] = useState(formatEditableModelPrice(model.inputMicroUsdPerMillion));
  const [cachedInputPrice, setCachedInputPrice] = useState(formatEditableModelPrice(model.cachedInputMicroUsdPerMillion ?? model.inputMicroUsdPerMillion));
  const [cacheWrite5mPrice, setCacheWrite5mPrice] = useState(formatEditableModelPrice(model.cacheWrite5mMicroUsdPerMillion ?? null));
  const [cacheWrite1hPrice, setCacheWrite1hPrice] = useState(formatEditableModelPrice(model.cacheWrite1hMicroUsdPerMillion ?? null));
  const [outputPrice, setOutputPrice] = useState(formatEditableModelPrice(model.outputMicroUsdPerMillion));
  const cacheWrite = supportsCacheWritePricing(model.id);
  const inputMicroUsd = parseEditableModelPrice(inputPrice);
  const cachedInputMicroUsd = parseEditableModelPrice(cachedInputPrice);
  const cacheWrite5mMicroUsd = parseOptionalEditableModelPrice(cacheWrite5mPrice);
  const cacheWrite1hMicroUsd = parseOptionalEditableModelPrice(cacheWrite1hPrice);
  const outputMicroUsd = parseEditableModelPrice(outputPrice);
  const operation = `model-price-${model.id}`;
  const valid = inputMicroUsd != null && cachedInputMicroUsd != null && outputMicroUsd != null && cacheWrite5mMicroUsd !== undefined && cacheWrite1hMicroUsd !== undefined;
  const setPrice = (input: number | null, cachedInput: number | null, write5m: number | null, write1h: number | null, output: number | null) => mode === "local"
    ? relayCommands.setModelPrice(model.id, input, cachedInput, write5m, write1h, output)
    : relayCommands.remoteAction({ type: "set_model_price" }, { modelId: model.id, inputMicroUsdPerMillion: input, cachedInputMicroUsdPerMillion: cachedInput, cacheWrite5mMicroUsdPerMillion: write5m, cacheWrite1hMicroUsdPerMillion: write1h, outputMicroUsdPerMillion: output });
  const save = async () => {
    if (!valid) return;
    const ok = await perform(operation, () => setPrice(inputMicroUsd, cachedInputMicroUsd, cacheWrite ? cacheWrite5mMicroUsd ?? null : null, cacheWrite ? cacheWrite1hMicroUsd ?? null : null, outputMicroUsd), "feedback.saved");
    if (ok) onClose();
  };
  const restore = async () => {
    const ok = await perform(operation, () => setPrice(null, null, null, null, null), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("models.priceTitle")} onClose={onClose} footer={<>{model.customPrice ? <Button className="model-price-restore" variant="secondary" icon={<RotateCcw aria-hidden />} disabled={busy === operation} onClick={() => void restore()}>{t("models.restorePrice")}</Button> : null}<Button variant="secondary" disabled={busy === operation} onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === operation} disabled={!valid} onClick={() => void save()}>{t("common.save")}</Button></>}>
    <div className="relay-form model-price-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.priceUnit")}</span></div>
      <section className="model-price-section">
        <h3>{t("models.tokenPrices")}</h3>
        <div className="model-price-grid">
          <ModelPriceField label={t("models.inputPriceLabel")} value={inputPrice} invalid={inputPrice.length > 0 && inputMicroUsd == null} onChange={setInputPrice} />
          <ModelPriceField label={t("models.outputPriceLabel")} value={outputPrice} invalid={outputPrice.length > 0 && outputMicroUsd == null} onChange={setOutputPrice} />
          <ModelPriceField label={t("models.cachedInputPriceLabel")} value={cachedInputPrice} invalid={cachedInputPrice.length > 0 && cachedInputMicroUsd == null} onChange={setCachedInputPrice} />
        </div>
      </section>
      {cacheWrite ? <section className="model-price-section">
        <h3>{t("models.cachePrices")}</h3>
        <div className="model-price-grid ttl">
          <ModelPriceField label={t("models.cacheWrite5mPriceLabel")} value={cacheWrite5mPrice} invalid={cacheWrite5mMicroUsd === undefined} onChange={setCacheWrite5mPrice} />
          <ModelPriceField label={t("models.cacheWrite1hPriceLabel")} value={cacheWrite1hPrice} invalid={cacheWrite1hMicroUsd === undefined} onChange={setCacheWrite1hPrice} />
        </div>
      </section> : null}
    </div>
  </Dialog>;
}

function ModelPriceField({ label, value, invalid, onChange }: { label: string; value: string; invalid: boolean; onChange: (value: string) => void }) {
  return <label className="model-price-field"><span className="model-price-label">{label}</span><span className="model-price-input"><span className="model-price-currency" aria-hidden>$</span><input aria-label={label} type="text" inputMode="decimal" autoComplete="off" spellCheck={false} value={value} placeholder="0.00" aria-invalid={invalid || undefined} onChange={(event) => onChange(event.target.value)} /></span></label>;
}
