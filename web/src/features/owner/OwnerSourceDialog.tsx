import { useId, useState } from "react";
import { Dialog } from "../../components/Dialog";
import { OptionMenu } from "../../components/OptionMenu";
import { Plus, X } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { sourceEndpoint, sourceModelProtocols, sourceProtocolLabels, validSourceConfiguration, validSourceEndpoint, validSourceModelId } from "./sourcePresentation";
import type { OwnerSourceConfiguration } from "./sourcePresentation";
import type { ConnectionAction, OwnerConnection } from "./model";
import "./owner-source-editor.css";

export function OwnerSourceDialog({ connection, initialSection = "connection", change, onClose }: {
  connection?: OwnerConnection | undefined; initialSection?: "connection" | "models" | undefined;
  change: (action: ConnectionAction) => void; onClose: () => void;
}) {
  const { t } = useLocale();
  const [id] = useState(() => connection?.id ?? crypto.randomUUID());
  const [section, setSection] = useState(initialSection);
  const [draft, setDraft] = useState<OwnerSourceConfiguration>(() => ({ name: connection?.name ?? "", baseUrl: sourceEndpoint(connection?.source?.baseUrl) ?? "", wireApi: connection?.source?.wireApi ?? "responses", models: [...connection?.models ?? []] }));
  const [model, setModel] = useState("");
  const [touched, setTouched] = useState(false);
  const hintId = useId();
  const duplicate = draft.models.some(id => id.toLowerCase() === model.trim().toLowerCase());
  const canAdd = validSourceModelId(model.trim()) && !duplicate && draft.models.length < 256;
  const unchangedTarget = draft.baseUrl === sourceEndpoint(connection?.source?.baseUrl) && draft.wireApi === connection?.source?.wireApi;
  function addModel() { if (canAdd) { setDraft(value => ({ ...value, models: [...value.models, model.trim()] })); setModel(""); } }
  return <Dialog className="owner-source-dialog" title={t(connection ? "source.editTitle" : "source.addTitle")} onClose={onClose}>
    <form className="owner-source-form" onSubmit={event => { event.preventDefault(); if (!validSourceConfiguration(draft)) { setSection("connection"); setTouched(true); return; } change({ type: "saveSource", id, configuration: { ...draft, baseUrl: draft.baseUrl.trim() } }); onClose(); }}>
      <div className="relay-tabs owner-source-tabs" role="group" aria-label={t("source.sections")}>
        {(["connection", "models"] as const).map(value => <button type="button" key={value} aria-pressed={section === value} className={section === value ? "active" : ""} onClick={() => setSection(value)}>{t(`source.section.${value}`)}</button>)}
      </div>
      <div className="owner-source-fields" hidden={section !== "connection"}>
        <label>{t("common.name")}<input data-initial-focus={initialSection === "connection" || undefined} maxLength={80} value={draft.name} onChange={event => setDraft(value => ({ ...value, name: event.target.value }))} autoComplete="off" /></label>
        <label>{t("source.endpoint")}<input type="url" value={draft.baseUrl} placeholder="https://api.example.com/v1" maxLength={2048} autoComplete="off" spellCheck={false} aria-invalid={touched && !validSourceEndpoint(draft.baseUrl) || undefined} aria-describedby={touched && !validSourceEndpoint(draft.baseUrl) ? hintId : undefined} onBlur={() => setTouched(true)} onChange={event => setDraft(value => ({ ...value, baseUrl: event.target.value }))} /></label>
        {touched && !validSourceEndpoint(draft.baseUrl) && <p className="form-note source-form-error" id={hintId} role="alert">{t("source.endpointInvalid")}</p>}
        <div className="owner-source-protocol-field"><span>{t("source.protocol")}</span><OptionMenu label={t("source.protocol")} value={draft.wireApi} options={Object.entries(sourceProtocolLabels).map(([value, label]) => ({ value, label }))} onChange={wireApi => setDraft(value => ({ ...value, wireApi: wireApi as OwnerSourceConfiguration["wireApi"] }))} /></div>
      </div>
      <div className="owner-source-model-editor" hidden={section !== "models"}>
        <div className="owner-source-model-input"><label className="sr-only" htmlFor={`${hintId}-model`}>{t("source.modelId")}</label><input id={`${hintId}-model`} data-initial-focus={initialSection === "models" || undefined} value={model} maxLength={200} placeholder={t("source.modelId")} autoComplete="off" spellCheck={false} onChange={event => setModel(event.target.value)} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addModel(); } }} /><button type="button" className="icon-button" aria-label={t("source.addModel")} disabled={!canAdd} onClick={addModel}><Plus aria-hidden="true" /></button></div>
        {duplicate && <p className="form-note" role="status">{t("source.modelExists")}</p>}
        <ul className="owner-source-model-list">{draft.models.map(id => {
          const protocols = sourceModelProtocols(unchangedTarget ? connection?.source : undefined, connection?.models ?? [], id);
          return <li key={id}><div><code>{id}</code><span>{protocols.length ? protocols.join(" · ") : t("source.protocolUnknown")}</span></div><button type="button" className="icon-button" aria-label={t("source.removeModel", { name: id })} onClick={() => setDraft(value => ({ ...value, models: value.models.filter(model => model !== id) }))}><X aria-hidden="true" /></button></li>;
        })}</ul>
        {!draft.models.length && <p className="owner-resource-empty">{t("source.noModels")}</p>}
      </div>
      <p className="form-note owner-source-preview-note">{t("source.previewOnly")}</p>
      <div className="button-row owner-source-form-actions"><button type="button" className="button" onClick={onClose}>{t("common.cancel")}</button><button type="submit" className="button primary-button" disabled={!validSourceConfiguration(draft)}>{t(connection ? "common.save" : "common.add")}</button></div>
    </form>
  </Dialog>;
}
