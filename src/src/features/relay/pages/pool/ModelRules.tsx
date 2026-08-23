import { useRef, useState } from "react";
import { BrainCircuit, CircleAlert, FlaskConical, Loader2, Pencil, Power, PowerOff, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ModelReasoningProbeResult, ModelSummary } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, StatusIcon } from "../../components/Ui";
import { modelProviderGroup, modelProviderGroupLabel, supportsCacheWritePricing } from "../../modelGroups";
import { formatEditableModelPrice, parseEditableModelPrice, parseOptionalEditableModelPrice } from "../../modelPricing";
import { groupModelSummariesForLauncher, modelSummaries } from "../../poolHelpers";
import {
  formatModelPrice,
  formatReasoningEffort,
  sortReasoningEfforts,
} from "../../poolFormatting";
import {
  addReasoningLevel,
  initialReasoningLevels,
  mergeSuccessfulProbe,
  normalizeReasoningLevel,
  toggleReasoningLevel,
} from "./modelReasoningPolicy";
import { useRelayState } from "../../state/RelayStateProvider";

export function ModelRulesView() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [priceModel, setPriceModel] = useState<ModelSummary | null>(null);
  const [reasoningModel, setReasoningModel] = useState<ModelSummary | null>(null);
  const models = runtime ? modelSummaries(runtime) : [];
  const modelGroups = groupModelSummariesForLauncher(models, runtime?.accounts ?? []);
  const discoveryErrors = runtime
    ? [...new Set([
      ...runtime.accounts
        .filter((account) => account.lastErrorCode?.trim().startsWith("models_"))
        .map((account) => `${account.label}: ${account.lastErrorCode!.trim()}`),
      ...runtime.sources
        .filter((source) => Boolean(source.lastErrorCode?.trim()))
        .map((source) => `${source.name}: ${source.lastErrorCode!.trim()}`),
      ...runtime.warnings
        .filter((warning) => warning.startsWith("model_catalog_refresh_failed:"))
        .map((warning) => warning.replace("model_catalog_refresh_failed:", "model catalog: ")),
    ])]
    : [];
  const catalogRefreshDeferred = runtime?.warnings.includes("model_catalog_refresh_deferred:codex_running") ?? false;
  const discoveryAlert = discoveryErrors.length || catalogRefreshDeferred
    ? <div className={`model-discovery-alert${catalogRefreshDeferred && !discoveryErrors.length ? " deferred" : ""}`} role={discoveryErrors.length ? "alert" : "status"}><CircleAlert aria-hidden /><span><strong>{discoveryErrors.length ? t("models.discoveryError") : t("models.discoveryDeferred")}</strong><small>{discoveryErrors.length ? t("models.discoveryErrorDetail", { errors: discoveryErrors.join(" · ") }) : t("models.discoveryDeferredDetail")}</small></span></div>
    : null;
  const canEditPrice = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("model_pricing"));
  const toggleModel = (model: ModelSummary) => perform(
    `model-toggle-${model.id}`,
    () => mode === "local"
      ? relayCommands.setModelEnabled(model.id, !model.enabled)
      : relayCommands.remoteAction({ type: "set_model_enabled" }, { modelId: model.id, enabled: !model.enabled }),
    "feedback.saved",
  );
  if (!models.length) return <div className="model-rules-empty">{discoveryAlert}<EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} /></div>;
  return <>{discoveryAlert}<section className="model-rules relay-compact-content">
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
      const imagePrices = model.imageRequestPrices ?? [];
      const isImageModel = imagePrices.length > 0;
      const hasPrice = !isImageModel && model.inputMicroUsdPerMillion != null && model.outputMicroUsdPerMillion != null;
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
      const hasReasoningModes = !isImageModel && ((model.reasoningLevels?.length ?? 0) > 0 || (model.reasoningSupportedLevels?.length ?? 0) > 0);
      const canEditReasoning = !isImageModel && Boolean(model.reasoningConfigurable);
      return <tr key={model.id} data-model-id={model.id} data-enabled={model.enabled ? "true" : "false"}>
        <td data-column="model"><div className="model-rule-identity"><strong title={displayName}>{displayName}</strong>{displayName !== model.id ? <code title={model.id}>{model.id}</code> : null}<span className={`model-rule-state ${model.enabled ? "ready" : "disabled"}`}><StatusIcon status={model.enabled ? "ready" : "disabled"} label={t(model.enabled ? "models.available" : "models.disabled")} /><span>{t(model.enabled ? "models.available" : "models.disabled")}</span></span></div></td>
        <td data-column="codex"><div className={`model-codex-state ${model.codexVisible ? "visible" : "hidden"}`}><BrainCircuit aria-hidden /><span><strong>{t(model.codexVisible ? "models.codexVisible" : model.enabled ? "models.codexUnsupported" : "models.codexDisabled")}</strong></span></div></td>
        <td data-column="price"><div className="model-price">{isImageModel ? <div className="model-image-price-summary"><span className="model-image-price-heading"><strong>{t("models.imageOperation.generation")}</strong><small>{t("models.imagePriceUnit")}</small></span><div className="model-image-price-list">{imagePrices.filter((price) => price.operation === "generation").slice(0, 3).map((price) => <span className="model-image-price-item" key={`${price.quality}-${price.size}`}><small>{t(`models.imageQuality.${price.quality}`, { defaultValue: price.quality })} · {price.size}</small><strong>{formatModelPrice(price.microUsd, i18n.language)}</strong></span>)}</div></div> : hasPrice ? <>{priceParts.map((part) => <span className="model-price-value" key={part.label}><small>{part.label}</small><strong>{part.value}</strong></span>)}{model.customPrice ? <small className="model-price-note custom">{t("models.customPrice")}</small> : null}</> : <span className="model-price-empty muted">{t("models.priceUnavailable")}</span>}</div></td>
        <td data-column="members"><span className="model-members">{t("pool.membersCount", { count: model.memberCount })}</span></td>
        <td data-column="actions"><div className="model-rule-actions">{(canEditPrice || isImageModel) ? <IconButton data-model-price-edit={model.id} label={t(isImageModel ? "models.viewImagePrice" : "models.editPrice", { model: model.id })} icon={<Pencil aria-hidden />} onClick={() => setPriceModel(model)} /> : null}{canEditReasoning || hasReasoningModes ? <IconButton data-model-reasoning-edit={model.id} label={t(canEditReasoning ? "models.editReasoning" : "models.viewReasoning", { model: model.id })} icon={<BrainCircuit aria-hidden />} onClick={() => setReasoningModel(model)} /> : null}<IconButton data-model-toggle={model.id} label={toggleLabel} icon={toggling ? <Loader2 className="spin" aria-hidden /> : <Power aria-hidden />} className="model-toggle" aria-pressed={model.enabled} disabled={toggling} onClick={() => void toggleModel(model)} /></div></td>
      </tr>;
    })}</tbody>)}
    </table></div>
  </section>{priceModel ? <ModelPriceDialog key={priceModel.id} model={priceModel} onClose={() => setPriceModel(null)} /> : null}{reasoningModel ? <ModelReasoningDialog key={reasoningModel.id} model={reasoningModel} onClose={() => setReasoningModel(null)} /> : null}</>;
}

function ModelReasoningDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const supportedLevels = model.reasoningSupportedLevels ?? [];
  const editable = Boolean(model.reasoningConfigurable);
  const companyGroup = modelProviderGroup(model.id);
  const [allowedLevels, setAllowedLevels] = useState(() => initialReasoningLevels(model.reasoningAllowedLevels, model.reasoningLevels ?? []));
  const allowedLevelsRef = useRef(allowedLevels);
  const policyRevision = useRef(0);
  const mutationLock = useRef(false);
  const [mutationInFlight, setMutationInFlight] = useState(false);
  const [customLevel, setCustomLevel] = useState("");
  const [probeLevel, setProbeLevel] = useState(allowedLevels[0] ?? supportedLevels[0] ?? "medium");
  const [addSuccessfulProbe, setAddSuccessfulProbe] = useState(false);
  const [probeResult, setProbeResult] = useState<ModelReasoningProbeResult | null>(null);
  const operation = `model-reasoning-${model.id}`;
  const probeOperation = `model-reasoning-probe-${model.id}`;
  const candidates = sortReasoningEfforts([...supportedLevels, ...allowedLevels, probeLevel]);
  const label = (level: string) => t(`usage.reasoningEfforts.${level}`, { defaultValue: formatReasoningEffort(level) });
  const updateAllowedLevels = (next: string[]) => {
    allowedLevelsRef.current = next;
    setAllowedLevels(next);
  };
  const runSerialized = async (id: string, work: () => Promise<unknown>, successKey?: string) => {
    if (mutationLock.current) return false;
    mutationLock.current = true;
    setMutationInFlight(true);
    try {
      return await perform(id, work, successKey);
    } finally {
      mutationLock.current = false;
      setMutationInFlight(false);
    }
  };
  const saveLevels = async (next: string[]) => {
    if (mutationLock.current) return;
    const normalized = sortReasoningEfforts(next);
    const revision = ++policyRevision.current;
    const ok = await runSerialized(operation, () => mode === "local"
      ? relayCommands.setModelReasoning(model.id, normalized)
      : relayCommands.remoteAction({ type: "set_model_reasoning" }, { modelId: model.id, allowedLevels: normalized }), "feedback.saved");
    if (ok && revision === policyRevision.current) updateAllowedLevels(normalized);
  };
  const toggleAllowedLevel = (level: string) => {
    const next = toggleReasoningLevel(allowedLevelsRef.current, level);
    if (next === allowedLevelsRef.current) return;
    void saveLevels(next);
  };
  const addLevel = () => {
    const next = addReasoningLevel(allowedLevelsRef.current, customLevel);
    if (next === allowedLevelsRef.current) return;
    setCustomLevel("");
    void saveLevels(next);
  };
  const runProbe = async () => {
    const level = normalizeReasoningLevel(probeLevel);
    if (!level || mode !== "local") return;
    const revision = policyRevision.current;
    await runSerialized(probeOperation, async () => {
      const result = await relayCommands.probeModelReasoning(model.id, level, addSuccessfulProbe);
      setProbeResult(result);
      if (revision === policyRevision.current) {
        updateAllowedLevels(mergeSuccessfulProbe(allowedLevelsRef.current, result));
      }
    });
  };
  const probing = mutationInFlight || busy === probeOperation;
  const manualBusy = mutationInFlight || busy === operation;
  return <Dialog title={t("models.reasoningTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
    <div className="model-reasoning-form">
      <div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t(editable ? "models.reasoningHint" : "models.reasoningReadOnlyHint")}</span></div>
      {editable ? <div className="model-reasoning-company"><span>{t("models.reasoningCompanyDefault")}</span><strong>{t(`modelGroups.${companyGroup}`, { defaultValue: modelProviderGroupLabel(companyGroup) })}</strong></div> : null}
      {editable ? <>
        <section className="model-reasoning-manual">
          <header className="model-reasoning-section-heading"><strong>{t("models.reasoningManual")}</strong><Button className="model-reasoning-clear" variant="danger" icon={<PowerOff aria-hidden />} disabled={manualBusy || probing || allowedLevels.length === 0} onClick={() => void saveLevels([])}>{t("models.reasoningClear")}</Button></header>
          <div className="model-reasoning-options" role="group" aria-label={t("models.reasoningManual")}>
            {candidates.map((level) => <button key={level} type="button" role="checkbox" aria-checked={allowedLevels.includes(level)} className={allowedLevels.includes(level) ? "selected" : undefined} disabled={manualBusy || probing} onClick={() => toggleAllowedLevel(level)}>{label(level)}</button>)}
          </div>
          <div className="model-reasoning-add"><label className="relay-field"><span>{t("models.reasoningCustom")}</span><input value={customLevel} disabled={manualBusy || probing} onChange={(event) => setCustomLevel(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addLevel(); } }} placeholder={t("models.reasoningCustomPlaceholder")} /></label><Button variant="secondary" disabled={!normalizeReasoningLevel(customLevel) || manualBusy || probing} onClick={addLevel}>{t("models.reasoningAdd")}</Button></div>
        </section>
        {mode === "local" && model.reasoningProbeAvailable ? <section className="model-reasoning-probe">
          <header><strong>{t("models.reasoningProbeTitle")}</strong></header>
          <div className="model-reasoning-probe-controls"><label className="relay-field"><span>{t("models.reasoningProbeCandidate")}</span><input aria-label={t("models.reasoningProbeCandidate")} list={`reasoning-candidates-${model.id}`} value={probeLevel} disabled={manualBusy || probing} onChange={(event) => setProbeLevel(event.target.value)} /><datalist id={`reasoning-candidates-${model.id}`}>{candidates.map((level) => <option key={level} value={level}>{label(level)}</option>)}</datalist></label><Button variant="secondary" icon={<FlaskConical aria-hidden />} busy={probing} disabled={!normalizeReasoningLevel(probeLevel) || manualBusy || probing} onClick={() => void runProbe()}>{t("models.reasoningProbeAction")}</Button></div>
          <label className="model-reasoning-probe-toggle"><input type="checkbox" checked={addSuccessfulProbe} disabled={manualBusy || probing} onChange={(event) => setAddSuccessfulProbe(event.target.checked)} /><span><strong>{t("models.reasoningProbeApply")}</strong></span></label>
          {probing ? <div className="model-reasoning-probe-progress" role="status"><Loader2 className="spin" aria-hidden /><span>{t("models.reasoningProbing")}</span></div> : null}
          {probeResult ? <div className="model-reasoning-probe-result" role="status"><strong>{t("models.reasoningProbeAvailability", probeResult)}</strong><ul>{probeResult.sources.map((source) => <li key={source.sourceId} data-available={source.available ? "true" : "false"}><span>{source.sourceName}</span><small>{t(source.available ? "models.reasoningProbeAvailable" : "models.reasoningProbeUnavailable")}</small></li>)}</ul>{probeResult.appliedToSettings ? <small>{t("models.reasoningProbeApplied")}</small> : null}</div> : null}
        </section> : null}
      </> : <div className="model-reasoning-detected"><span>{t("models.reasoningAvailable")}</span><strong>{(model.reasoningLevels ?? []).length ? (model.reasoningLevels ?? []).map(label).join(", ") : t("models.reasoningDetectedEmpty")}</strong></div>}
    </div>
  </Dialog>;
}

function ModelPriceDialog({ model, onClose }: { model: ModelSummary; onClose: () => void }) {
  const { t, i18n } = useTranslation();
  const imagePrices = model.imageRequestPrices ?? [];
  if (imagePrices.length) {
    return <Dialog title={t("models.imagePriceTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.close")}</Button>}>
      <div className="relay-form model-price-form"><div className="model-price-context"><code title={model.id}>{model.id}</code><span>{t("models.imagePriceUnit")}</span></div>
        {["generation", "edit"].map((operation) => <section className="model-price-section" key={operation}><h3>{t(`models.imageOperation.${operation}`)}</h3><div className="model-price-grid">{imagePrices.filter((price) => price.operation === operation).map((price) => <div className="model-price-value" key={`${operation}-${price.quality}-${price.size}`}><small>{t(`models.imageQuality.${price.quality}`, { defaultValue: price.quality })} · {price.size}</small><strong>{formatModelPrice(price.microUsd, i18n.language)}</strong></div>)}</div></section>)}
      </div>
    </Dialog>;
  }
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
