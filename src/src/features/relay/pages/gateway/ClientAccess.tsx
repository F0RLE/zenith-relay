import { useState } from "react";
import { Check, Copy, KeyRound, Pencil, Plus, Power, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ClientKeyCreateInput, ClientWireApi, GeneratedClientKey, KeySummary, RuntimeSnapshot } from "../../api/types";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, Button, Dialog, IconButton, StatusIcon, copyText } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type EditorTarget = "create" | KeySummary | null;
type Confirmation = { kind: "rotate" | "delete"; key: KeySummary } | null;
type ModelPolicy = "all" | "allow" | "exclude";

export function RemoteClientAccess() {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const [editor, setEditor] = useState<EditorTarget>(null);
  const [confirmation, setConfirmation] = useState<Confirmation>(null);
  const [generated, setGenerated] = useState<GeneratedClientKey | null>(null);
  if (!runtime?.capabilities.features.includes("client_access")) return null;

  const keys = runtime.keys.filter((key) => !key.system);
  const supportedApis = clientApis(runtime);
  const save = async (input: ClientKeyCreateInput) => {
    if (editor === "create") {
      let result: GeneratedClientKey | null = null;
      const ok = await perform("remote-client-create", async () => {
        result = await relayCommands.createRemoteClientKey(input);
      }, "feedback.saved");
      if (ok && result) {
        setEditor(null);
        setGenerated(result);
      }
      return;
    }
    if (!editor) return;
    const ok = await perform(`remote-client-update-${editor.id}`, () => relayCommands.updateRemoteClientKey(editor.id, input), "feedback.saved");
    if (ok) setEditor(null);
  };
  const setEnabled = (key: KeySummary) => perform(
    `remote-client-toggle-${key.id}`,
    () => relayCommands.updateRemoteClientKey(key.id, { schemaVersion: 1, enabled: !key.enabled }),
    "feedback.saved",
  );
  const confirm = async () => {
    if (!confirmation) return;
    const { kind, key } = confirmation;
    if (kind === "delete") {
      const ok = await perform(`remote-client-delete-${key.id}`, () => relayCommands.revokeRemoteClientKey(key.id), "feedback.saved");
      if (ok) setConfirmation(null);
      return;
    }
    let result: GeneratedClientKey | null = null;
    const ok = await perform(`remote-client-rotate-${key.id}`, async () => {
      result = await relayCommands.rotateRemoteClientKey(key.id);
    }, "feedback.saved");
    if (ok && result) {
      setConfirmation(null);
      setGenerated(result);
    }
  };

  return <>
    <section className="gateway-setting-row gateway-client-access">
      <header><span className="gateway-config-icon"><KeyRound aria-hidden /></span><div><h2>{t("gateway.clientAccess.title")}</h2><p>{t("gateway.clientAccess.hint")}</p></div></header>
      <div className="client-access-content">
        <div className="client-access-toolbar"><span>{t("gateway.clientAccess.keyCount", { count: keys.length })}</span><Button variant="secondary" icon={<Plus aria-hidden />} disabled={!supportedApis.length} onClick={() => setEditor("create")}>{t("gateway.clientAccess.create")}</Button></div>
        {keys.length ? <div className="client-key-list">{keys.map((key) => <ClientKeyRow key={key.id} keyRecord={key} runtime={runtime} supportedApis={supportedApis} busy={busy} onEdit={() => setEditor(key)} onToggle={() => void setEnabled(key)} onConfirm={(kind) => setConfirmation({ kind, key })} />)}</div> : <div className="client-access-empty"><KeyRound aria-hidden /><div><strong>{t("gateway.clientAccess.empty")}</strong><small>{t("gateway.clientAccess.emptyHint")}</small></div></div>}
      </div>
    </section>
    {editor ? <ClientKeyDialog key={editor === "create" ? "create" : editor.id} keyRecord={editor === "create" ? null : editor} runtime={runtime} supportedApis={supportedApis} busy={busy} onClose={() => setEditor(null)} onSave={(input) => void save(input)} /> : null}
    {confirmation ? <Dialog title={t(`gateway.clientAccess.${confirmation.kind}Title`)} onClose={() => setConfirmation(null)} footer={<><Button variant="secondary" onClick={() => setConfirmation(null)}>{t("common.cancel")}</Button><Button variant={confirmation.kind === "delete" ? "danger" : "primary"} busy={busy === `remote-client-${confirmation.kind}-${confirmation.key.id}`} onClick={() => void confirm()}>{t(`gateway.clientAccess.${confirmation.kind}`)}</Button></>}><p className="confirm-dialog-message">{t(`gateway.clientAccess.${confirmation.kind}Confirm`, { label: confirmation.key.label })}</p></Dialog> : null}
    {generated ? <GeneratedSecretDialog generated={generated} endpoint={runtime.gateway.baseUrl} onClose={() => setGenerated(null)} /> : null}
  </>;
}

function ClientKeyRow({ keyRecord, runtime, supportedApis, busy, onEdit, onToggle, onConfirm }: {
  keyRecord: KeySummary;
  runtime: RuntimeSnapshot;
  supportedApis: ClientWireApi[];
  busy: string | null;
  onEdit: () => void;
  onToggle: () => void;
  onConfirm: (kind: "rotate" | "delete") => void;
}) {
  const { t, i18n } = useTranslation();
  const apis = keyRecord.wireApis ?? supportedApis;
  const modelCount = keyRecord.allowedModels.length || (keyRecord.excludedModels.length ? Math.max(0, runtime.gateway.visibleModelIds.length - keyRecord.excludedModels.length) : runtime.gateway.visibleModelIds.length);
  const spent = keyRecord.usageTotals?.apiEquivalent.microUsd ?? 0;
  const budget = keyRecord.softBudgetMicroUsd ?? null;
  return <div className={`client-key-row${keyRecord.enabled ? "" : " disabled"}`}>
    <div className="client-key-identity"><StatusIcon status={keyRecord.enabled ? "ready" : "disabled"} label={keyRecord.enabled ? t("common.enabled") : t("common.disabled")} /><span><strong>{keyRecord.label}</strong><small><code>zrs_********</code> · {keyRecord.id}</small></span></div>
    <div className="client-key-scopes">
      <span>{t("gateway.clientAccess.protocolsShort")}: <strong>{apis.map((api) => t(`gateway.clientAccess.protocols.${api}`)).join(", ")}</strong></span>
      <span>{t("gateway.clientAccess.sourcesShort")}: <strong>{scopeLabel(keyRecord.sourceIds, runtime.sources.length, t("common.all"))}</strong></span>
      <span>{t("gateway.clientAccess.accountsShort")}: <strong>{scopeLabel(keyRecord.accountIds, runtime.accounts.length, t("common.all"))}</strong></span>
      <span>{t("common.models")}: <strong>{keyRecord.allowedModels.length || keyRecord.excludedModels.length ? modelCount : t("common.all")}</strong></span>
      {keyRecord.usageTotals ? <span className={budget != null && spent >= budget ? "over-budget" : undefined}>{t("gateway.clientAccess.usageShort")}: <strong>{formatMicroUsd(spent, i18n.resolvedLanguage ?? i18n.language)}{budget != null ? ` / ${formatMicroUsd(budget, i18n.resolvedLanguage ?? i18n.language)}` : ""} · {t("gateway.clientAccess.requestCount", { count: keyRecord.usageTotals.requests })}</strong></span> : null}
    </div>
    <div className="client-key-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} disabled={Boolean(busy)} onClick={onEdit} /><ActionMenu><ActionMenuItem icon={<RefreshCw aria-hidden />} disabled={Boolean(busy)} onClick={() => onConfirm("rotate")}>{t("gateway.clientAccess.rotate")}</ActionMenuItem><ActionMenuItem icon={<Power aria-hidden />} disabled={Boolean(busy)} onClick={onToggle}>{keyRecord.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem><ActionMenuItem icon={<Trash2 aria-hidden />} danger disabled={Boolean(busy)} onClick={() => onConfirm("delete")}>{t("common.delete")}</ActionMenuItem></ActionMenu></div>
  </div>;
}

function ClientKeyDialog({ keyRecord, runtime, supportedApis, busy, onClose, onSave }: {
  keyRecord: KeySummary | null;
  runtime: RuntimeSnapshot;
  supportedApis: ClientWireApi[];
  busy: string | null;
  onClose: () => void;
  onSave: (input: ClientKeyCreateInput) => void;
}) {
  const { t } = useTranslation();
  const [label, setLabel] = useState(keyRecord?.label ?? "");
  const [allSources, setAllSources] = useState(keyRecord?.sourceIds == null);
  const [sourceIds, setSourceIds] = useState(keyRecord?.sourceIds ?? []);
  const [allAccounts, setAllAccounts] = useState(keyRecord?.accountIds == null);
  const [accountIds, setAccountIds] = useState(keyRecord?.accountIds ?? []);
  const initialModelPolicy: ModelPolicy = keyRecord?.allowedModels.length ? "allow" : keyRecord?.excludedModels.length ? "exclude" : "all";
  const [modelPolicy, setModelPolicy] = useState<ModelPolicy>(initialModelPolicy);
  const [modelIds, setModelIds] = useState(initialModelPolicy === "allow" ? keyRecord?.allowedModels ?? [] : initialModelPolicy === "exclude" ? keyRecord?.excludedModels ?? [] : []);
  const [modelPrefix, setModelPrefix] = useState(keyRecord?.modelPrefix ?? "");
  const [wireApis, setWireApis] = useState<ClientWireApi[]>(keyRecord?.wireApis ?? supportedApis);
  const supportsBudgets = runtime.capabilities.features.includes("client_key_budgets");
  const [budget, setBudget] = useState(formatEditableMicroUsd(keyRecord?.softBudgetMicroUsd ?? null));
  const budgetMicroUsd = parseUsdMicro(budget);
  const modelOptions = [...new Set([...(runtime.gateway.models?.map((model) => model.id) ?? []), ...runtime.gateway.visibleModelIds])].sort();
  const valid = label.trim().length > 0 && wireApis.length > 0 && (modelPolicy === "all" || modelIds.length > 0) && (!supportsBudgets || budgetMicroUsd !== undefined);
  const toggle = (values: string[], value: string, setValues: (values: string[]) => void) => setValues(values.includes(value) ? values.filter((item) => item !== value) : [...values, value]);
  const submit = () => onSave({
    schemaVersion: 1,
    label: label.trim(),
    sourceIds: allSources ? null : sourceIds,
    accountIds: allAccounts ? null : accountIds,
    allowedModels: modelPolicy === "allow" ? modelIds : [],
    excludedModels: modelPolicy === "exclude" ? modelIds : [],
    modelPrefix: modelPrefix.trim() || null,
    wireApis,
    ...(supportsBudgets ? { softBudgetMicroUsd: budgetMicroUsd } : {}),
  });

  return <Dialog wide title={keyRecord ? t("gateway.clientAccess.editTitle") : t("gateway.clientAccess.createTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (keyRecord ? `remote-client-update-${keyRecord.id}` : "remote-client-create")} disabled={!valid} onClick={submit}>{t("common.save")}</Button></>}>
    <div className="relay-form client-key-form">
      <label className="relay-field"><span>{t("gateway.clientAccess.name")}</span><input autoFocus value={label} maxLength={128} onChange={(event) => setLabel(event.target.value)} placeholder={t("gateway.clientAccess.namePlaceholder")} /></label>
      {supportsBudgets ? <label className="relay-field"><span>{t("gateway.clientAccess.budget")}</span><input value={budget} inputMode="decimal" maxLength={16} onChange={(event) => setBudget(event.target.value)} placeholder={t("gateway.clientAccess.budgetPlaceholder")} />{budgetMicroUsd === undefined ? <small className="field-error">{t("gateway.clientAccess.budgetInvalid")}</small> : <small className="form-note">{t("gateway.clientAccess.budgetHint")}</small>}</label> : null}
      <fieldset className="client-access-fieldset"><legend>{t("gateway.clientAccess.protocolsTitle")}</legend><p>{t("gateway.clientAccess.protocolsHint")}</p><div className="client-protocol-grid">{supportedApis.map((api) => <label key={api} className={wireApis.includes(api) ? "selected" : ""}><input type="checkbox" checked={wireApis.includes(api)} onChange={() => toggle(wireApis, api, (values) => setWireApis(values as ClientWireApi[]))} /><span><strong>{t(`gateway.clientAccess.protocols.${api}`)}</strong><small>{t(`gateway.clientAccess.protocolHints.${api}`)}</small></span><Check aria-hidden /></label>)}</div>{!wireApis.length ? <small className="field-error">{t("gateway.clientAccess.protocolRequired")}</small> : null}</fieldset>
      <ScopeField title={t("gateway.clientAccess.sourcesTitle")} allLabel={t("gateway.clientAccess.allSources")} all={allSources} setAll={setAllSources} values={sourceIds} options={runtime.sources.map((source) => ({ id: source.id, label: source.name }))} setValues={setSourceIds} empty={t("gateway.clientAccess.noSources")} />
      <ScopeField title={t("gateway.clientAccess.accountsTitle")} allLabel={t("gateway.clientAccess.allAccounts")} all={allAccounts} setAll={setAllAccounts} values={accountIds} options={runtime.accounts.map((account) => ({ id: account.id, label: account.label }))} setValues={setAccountIds} empty={t("gateway.clientAccess.noAccounts")} />
      <fieldset className="client-access-fieldset"><legend>{t("gateway.clientAccess.modelsTitle")}</legend><div className="segmented client-model-policy" role="group" aria-label={t("gateway.clientAccess.modelsTitle")}>{(["all", "allow", "exclude"] as ModelPolicy[]).map((policy) => <button key={policy} type="button" className={modelPolicy === policy ? "active" : ""} aria-pressed={modelPolicy === policy} onClick={() => { setModelPolicy(policy); setModelIds([]); }}>{t(`gateway.clientAccess.modelPolicies.${policy}`)}</button>)}</div>{modelPolicy !== "all" ? <CheckGrid values={modelIds} options={modelOptions.map((model) => ({ id: model, label: model }))} setValues={setModelIds} empty={t("gateway.clientAccess.noModels")} /> : null}<label className="relay-field client-prefix-field"><span>{t("gateway.clientAccess.modelPrefix")}</span><input value={modelPrefix} onChange={(event) => setModelPrefix(event.target.value)} placeholder={t("gateway.clientAccess.modelPrefixPlaceholder")} /></label></fieldset>
    </div>
  </Dialog>;
}

function ScopeField({ title, allLabel, all, setAll, values, options, setValues, empty }: { title: string; allLabel: string; all: boolean; setAll: (value: boolean) => void; values: string[]; options: Array<{ id: string; label: string }>; setValues: (values: string[]) => void; empty: string }) {
  return <fieldset className="client-access-fieldset"><legend>{title}</legend><label className="toggle-row client-scope-all"><input type="checkbox" checked={all} onChange={(event) => setAll(event.target.checked)} /><span>{allLabel}</span></label>{!all ? <CheckGrid values={values} options={options} setValues={setValues} empty={empty} /> : null}</fieldset>;
}

function CheckGrid({ values, options, setValues, empty }: { values: string[]; options: Array<{ id: string; label: string }>; setValues: (values: string[]) => void; empty: string }) {
  if (!options.length) return <p className="form-note">{empty}</p>;
  return <div className="scope-grid client-scope-grid">{options.map((option) => <label key={option.id}><input type="checkbox" checked={values.includes(option.id)} onChange={() => setValues(values.includes(option.id) ? values.filter((id) => id !== option.id) : [...values, option.id])} /><span title={option.label}>{option.label}</span></label>)}</div>;
}

function GeneratedSecretDialog({ generated, endpoint, onClose }: { generated: GeneratedClientKey; endpoint: string; onClose: () => void }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const copy = async () => { await copyText(generated.secret); setCopied(true); };
  return <Dialog title={t("gateway.clientAccess.secretTitle")} onClose={onClose} footer={<Button variant="primary" onClick={onClose}>{t("common.done")}</Button>}><div className="client-secret-reveal"><KeyRound aria-hidden /><p>{t("gateway.clientAccess.secretHint")}</p><label><span>{t("gateway.endpoint")}</span><code>{endpoint}</code></label><label><span>{t("gateway.clientAccess.secret")}</span><div className="client-secret-value"><input value={generated.secret} readOnly spellCheck={false} /><Button variant="secondary" icon={copied ? <Check aria-hidden /> : <Copy aria-hidden />} onClick={() => void copy()}>{copied ? t("gateway.clientAccess.copied") : t("common.copy")}</Button></div></label><small>{t("gateway.clientAccess.secretOnce")}</small></div></Dialog>;
}

function clientApis(runtime: RuntimeSnapshot): ClientWireApi[] {
  const apis: ClientWireApi[] = [];
  if (runtime.capabilities.supportedWireApis?.includes("responses")) apis.push("responses");
  if (runtime.capabilities.supportedWireApis?.includes("chat_completions")) apis.push("chat_completions");
  if (runtime.capabilities.features.includes("images")) apis.push("images");
  return apis;
}

function scopeLabel(ids: string[] | null, total: number, all: string) {
  return ids == null ? all : `${ids.length}/${total}`;
}

function parseUsdMicro(value: string): number | null | undefined {
  const normalized = value.trim().replace(",", ".");
  if (!normalized) return null;
  if (!/^\d{1,7}(?:\.\d{1,6})?$/.test(normalized)) return undefined;
  const [whole, fraction = ""] = normalized.split(".");
  const microUsd = Number(whole) * 1_000_000 + Number(fraction.padEnd(6, "0"));
  return microUsd > 0 && microUsd <= 1_000_000_000_000 ? microUsd : undefined;
}

function formatEditableMicroUsd(value: number | null) {
  return value == null ? "" : (value / 1_000_000).toFixed(6).replace(/\.?0+$/, "");
}

function formatMicroUsd(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 6 }).format(value / 1_000_000);
}
