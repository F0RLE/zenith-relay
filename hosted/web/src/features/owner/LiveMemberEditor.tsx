import { useEffect, useId, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { changeMemberModels, readMemberModels, type MemberModels, type MemberModelsChange } from "../../api/memberModels";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { Dialog } from "../../components/Dialog";
import { Search } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import type { PoolMember } from "./livePool";
import { servingLabels, SourceServingStatus, type ServingAction } from "./SourceServing";
import "./owner-pool-member-editor.css";

const sections = ["models", "settings"] as const;
const labels = { models: "pool.memberModels", settings: "pool.memberSettings" } as const;

export function LiveMemberEditor({ member, csrf, blocked, close, saved, change }: {
  member: PoolMember; csrf: string; blocked: boolean;
  close: () => void; saved: () => void; change: (action: ServingAction) => void;
}) {
  const { t, locale } = useLocale();
  const id = useId();
  const [section, setSection] = useState<typeof sections[number]>("models");
  const [snapshot, setSnapshot] = useState<MemberModels>();
  const [draft, setDraft] = useState(new Map<string, boolean>());
  const [search, setSearch] = useState("");
  const [reload, setReload] = useState(0);
  const [error, setError] = useState<MessageKey>();
  const [readFailed, setReadFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState<MemberModelsChange>();
  const changing = useRef(false);

  useEffect(() => {
    const abort = new AbortController();
    void readMemberModels(member.id, abort.signal).then(value => {
      if (!abort.signal.aborted) setSnapshot(value);
    }, e => {
      if (abort.signal.aborted) return;
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      setReadFailed(true);
      setError(e instanceof ControlError && e.status === 404 ? "pool.memberModelsUnavailable" : "unavailable.server");
    });
    return () => abort.abort();
  }, [member.id, reload]);

  function reloadPolicy(note?: MessageKey) {
    setSnapshot(undefined);
    setDraft(new Map());
    setRetry(undefined);
    setReadFailed(false);
    setError(note);
    setReload(n => n + 1);
  }

  async function save() {
    if (!snapshot || !draft.size || blocked || changing.current) return;
    changing.current = true;
    setBusy(true);
    setError(undefined);
    const command = retry ?? {
      operation_id: crypto.randomUUID(),
      scope_id: snapshot.scope_id,
      expected_revision: snapshot.revision,
      expected_source_revision: snapshot.source_revision,
      changes: Array.from(draft, ([model, allowed]) => ({ model, allowed })),
    };
    try {
      await changeMemberModels(member.id, csrf, command);
      saved();
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && (e.code === "member_models_changed" || e.status === 404)) {
        reloadPolicy("pool.memberModelsChanged");
      } else if (e instanceof OwnerPasskeyError) {
        setRetry(undefined);
        setError("access.passkeyFailed");
      } else {
        // Retry with the same command so a committed write returns its receipt.
        setRetry(command);
        setError("catalog.retry");
      }
    } finally {
      changing.current = false;
      setBusy(false);
    }
  }

  const models = snapshot?.items ?? [];
  const count = models.filter(row => draft.get(row.model) ?? row.allowed).length;
  const shown = models.filter(row => row.model.toLowerCase().includes(search.trim().toLowerCase()));
  const { serving } = member;
  const pause = serving.state === "paused" ? "resume" : "pause";
  const settingsBlocked = blocked || busy || !!draft.size || snapshot?.source_revision !== serving.revision;
  const editBlocked = blocked || busy || !!retry;
  const canSave = !blocked && (readFailed || draft.size > 0);
  return <Dialog className="owner-member-dialog live-member-dialog" title={t("pool.editMember")} onClose={close} dismissible={!busy}>
    <div className="owner-member-identity">
      <strong>{member.label}</strong><SourceServingStatus view={serving} />
    </div>
    <div className="owner-member-tabs relay-tabs" role="tablist" aria-label={t("pool.editMember")} onKeyDown={event => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key) || busy) return;
      event.preventDefault();
      const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : section === "models" ? 1 : 0;
      setSection(sections[next]!);
      event.currentTarget.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
    }}>
      {sections.map(value => <button key={value} type="button" role="tab" id={`${id}-${value}`}
        aria-selected={section === value} aria-controls={`${id}-panel`} tabIndex={section === value ? 0 : -1}
        data-initial-focus={value === "models" ? "" : undefined} disabled={busy}
        className={section === value ? "active" : ""} onClick={() => setSection(value)}>{t(labels[value])}</button>)}
    </div>
    <div className="owner-member-panel" role="tabpanel" aria-labelledby={`${id}-${section}`} id={`${id}-panel`}>
      {section === "models" ? snapshot ? <section className="owner-member-models" aria-label={t("pool.allowedModels")}>
        <div className="owner-member-model-heading"><strong>{t("pool.allowedModels")}</strong>
          <span>{count.toLocaleString(locale)} / {models.length.toLocaleString(locale)}</span></div>
        {models.length > 6 && <label className="owner-member-search"><Search aria-hidden="true" />
          <input type="search" aria-label={t("pool.searchMemberModels")} placeholder={t("pool.searchMemberModels")}
            value={search} onChange={event => setSearch(event.target.value)} /></label>}
        {shown.length ? <div className="owner-member-model-group"><ul>{shown.map(({ model, allowed }) => {
          const checked = draft.get(model) ?? allowed;
          return <li key={model} data-enabled={checked}><label><code>{model}</code>
            <input type="checkbox" role="switch" className="owner-member-switch" aria-label={t("pool.allowMemberModel", { model })}
              checked={checked} disabled={editBlocked} onChange={event => {
                const next = event.target.checked;
                setError(undefined);
                setDraft(current => {
                  const changes = new Map(current);
                  if (allowed === next) changes.delete(model);
                  else changes.set(model, next);
                  return changes;
                });
              }} /></label></li>;
        })}</ul></div> : <p className="owner-member-empty" role="status">{t("pool.noMatchingModels")}</p>}
      </section> : !readFailed && <p className="owner-member-empty" role="status">{t("unavailable.connecting")}</p>
      : <div className="button-row live-member-settings">
        {!serving.draining && <button type="button" className="relay-button" disabled={settingsBlocked}
          onClick={() => change(pause)}>{t(servingLabels[pause])}</button>}
        <button type="button" className="relay-button" aria-haspopup="dialog" disabled={settingsBlocked}
          onClick={() => change("park")}>{t(servingLabels.park)}</button>
      </div>}
    </div>
    {error && <p className="owner-member-error" role="alert">{t(error)}</p>}
    {busy && <p className="sr-only" role="status">{t("unavailable.connecting")}</p>}
    <footer className="owner-member-footer">
      <button type="button" className="relay-button" disabled={busy} onClick={close}>{t("common.cancel")}</button>
      <button type="button" className="relay-button primary" disabled={!canSave}
        aria-disabled={busy || undefined} onClick={() => {
          if (busy) return;
          if (readFailed) reloadPolicy();
          else void save();
        }}>{t(readFailed || retry ? "common.retry" : "common.save")}</button>
    </footer>
  </Dialog>;
}
