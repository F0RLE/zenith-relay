import { useId, useState, type ReactNode } from "react";
import { qualifiedSourceEndpoint, validDeclaredModel } from "../../api/sourceIntake";
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
  const [id] = useState(() => connection?.id ?? crypto.randomUUID());
  return <SourceEditor connection={connection} initialSection={initialSection} onClose={onClose} onSave={configuration => {
    change({ type: "saveSource", id, configuration });
    onClose();
  }} />;
}

type Intake = { fields: ReactNode; ready: boolean; busy: boolean; feedback: ReactNode };

export function SourceEditor({ connection, initialSection = "connection", intake, onSave, onClose }: {
  connection?: OwnerConnection | undefined; initialSection?: "connection" | "models" | undefined;
  intake?: Intake; onSave: (configuration: OwnerSourceConfiguration) => void; onClose: () => void;
}) {
  const { t } = useLocale();
  const [section, setSection] = useState(initialSection);
  const [draft, setDraft] = useState<OwnerSourceConfiguration>(() => ({ name: connection?.name ?? "", baseUrl: sourceEndpoint(connection?.source?.baseUrl) ?? "", wireApi: connection?.source?.wireApi ?? "responses", models: [...connection?.models ?? []] }));
  const [model, setModel] = useState("");
  const [touched, setTouched] = useState(false);
  const hintId = useId();
  const busy = intake?.busy ?? false;
  const duplicate = draft.models.some(id => id.toLowerCase() === model.trim().toLowerCase());
  const canAdd = (intake ? validDeclaredModel : validSourceModelId)(model.trim())
    && !duplicate && draft.models.length < (intake ? 128 : 256);
  const endpoint = intake ? qualifiedSourceEndpoint(draft.baseUrl) : draft.baseUrl.trim();
  const invalidEndpoint = touched && !(intake ? endpoint : validSourceEndpoint(draft.baseUrl));
  const valid = validSourceConfiguration(draft) && !!endpoint && (!intake
    || (intake.ready && !/\p{Cc}/u.test(draft.name) && (!model.trim() || canAdd)));
  const unchangedTarget = draft.baseUrl === sourceEndpoint(connection?.source?.baseUrl) && draft.wireApi === connection?.source?.wireApi;
  function addModel() { if (canAdd && !busy) { setDraft(value => ({ ...value, models: [...value.models, model.trim()] })); setModel(""); } }
  return <Dialog className="owner-source-dialog" title={t(connection ? "source.editTitle" : "source.addTitle")} onClose={onClose} dismissible={!busy}>
    <form className="owner-source-form" aria-busy={busy || undefined} onSubmit={event => {
      event.preventDefault();
      if (busy || !valid || !endpoint) return;
      onSave({ ...draft, baseUrl: endpoint, models: intake && canAdd ? [...draft.models, model.trim()] : draft.models });
    }}>
      <div className="relay-tabs owner-source-tabs" role="group" aria-label={t("source.sections")}>
        {(["connection", "models"] as const).map(value => <button type="button" key={value} disabled={busy} aria-pressed={section === value} className={section === value ? "active" : ""} onClick={() => setSection(value)}>{t(`source.section.${value}`)}</button>)}
      </div>
      <div className="owner-source-fields" hidden={section !== "connection"}>
        <label>{t("common.name")}<input data-initial-focus={initialSection === "connection" || undefined} disabled={busy} maxLength={80} value={draft.name} onChange={event => setDraft(value => ({ ...value, name: event.target.value }))} autoComplete="off" /></label>
        <label>{t("source.endpoint")}<input type="url" disabled={busy} value={draft.baseUrl} placeholder="https://api.example.com/v1" maxLength={2048} autoComplete="off" spellCheck={false} aria-invalid={invalidEndpoint || undefined} aria-describedby={invalidEndpoint ? hintId : undefined} onBlur={() => setTouched(true)} onChange={event => setDraft(value => ({ ...value, baseUrl: event.target.value }))} /></label>
        {invalidEndpoint && <p className="form-note source-form-error" id={hintId} role="alert">{t(intake ? "sourceIntake.endpoint" : "source.endpointInvalid")}</p>}
        <div className="owner-source-protocol-field"><span>{t("source.protocol")}</span>{intake
          ? <span className="owner-source-protocol-value">{sourceProtocolLabels.responses}</span>
          : <OptionMenu label={t("source.protocol")} value={draft.wireApi} options={Object.entries(sourceProtocolLabels).map(([value, label]) => ({ value, label }))} onChange={wireApi => setDraft(value => ({ ...value, wireApi: wireApi as OwnerSourceConfiguration["wireApi"] }))} />}</div>
        {intake?.fields}
      </div>
      <div className="owner-source-model-editor" hidden={section !== "models"}>
        <div className="owner-source-model-input"><label className="sr-only" htmlFor={`${hintId}-model`}>{t("source.modelId")}</label><input id={`${hintId}-model`} data-initial-focus={initialSection === "models" || undefined} disabled={busy} value={model} maxLength={intake ? 128 : 200} placeholder={t("source.modelId")} autoComplete="off" spellCheck={false} onChange={event => setModel(event.target.value)} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addModel(); } }} /><button type="button" className="icon-button" aria-label={t("source.addModel")} disabled={!canAdd || busy} onClick={addModel}><Plus aria-hidden="true" /></button></div>
        {duplicate && <p className="form-note" role="status">{t("source.modelExists")}</p>}
        {intake && model.trim() && !canAdd && !duplicate && <p className="form-note" role="status">{t(draft.models.length >= 128 ? "sourceIntake.modelLimit" : "sourceIntake.modelInvalid")}</p>}
        <ul className="owner-source-model-list">{draft.models.map(id => {
          const protocols = sourceModelProtocols(unchangedTarget ? connection?.source : undefined, connection?.models ?? [], id);
          return <li key={id}><div><code>{id}</code><span>{protocols.length ? protocols.join(" · ") : t(intake ? "sourceIntake.unverified" : "source.protocolUnknown")}</span></div><button type="button" className="icon-button" disabled={busy} aria-label={t("source.removeModel", { name: id })} onClick={() => setDraft(value => ({ ...value, models: value.models.filter(model => model !== id) }))}><X aria-hidden="true" /></button></li>;
        })}</ul>
        {!draft.models.length && <p className="owner-resource-empty">{t("source.noModels")}</p>}
      </div>
      {intake ? intake.feedback : <p className="form-note owner-source-preview-note">{t("source.previewOnly")}</p>}
      <div className="button-row owner-source-form-actions">
        <button type="button" className="button" disabled={busy} onClick={onClose}>{t("common.cancel")}</button>
        <button type="submit" className="button primary-button" disabled={!valid || busy}>{t(busy ? "sourceIntake.saving" : connection || intake ? "common.save" : "common.add")}</button>
      </div>
    </form>
  </Dialog>;
}
