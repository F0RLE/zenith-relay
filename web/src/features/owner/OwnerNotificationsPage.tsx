import { useRef, useState } from "react";
import { Bell, Plus } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import "./owner-notifications.css";

interface NotificationDraft {
  id: string;
  title: string;
  message: string;
}

export function OwnerNotificationsPage() {
  const { t } = useLocale();
  const [drafts, setDrafts] = useState<NotificationDraft[]>([]);
  const [editor, setEditor] = useState<NotificationDraft | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const createButton = useRef<HTMLButtonElement>(null);
  const editButtons = useRef(new Map<string, HTMLButtonElement>());
  const returnTo = useRef<string | null>(null);

  function finishEditor() {
    const target = returnTo.current;
    setEditor(null);
    requestAnimationFrame(() => (target ? editButtons.current.get(target) : createButton.current)?.focus());
  }

  return <section className="owner-notifications-page">
    <header className="page-heading">
      <h1>{t("shell.notifications")}</h1>
      {!editor && <button ref={createButton} type="button" className="relay-button primary" onClick={() => {
        returnTo.current = null;
        setDeleting(null);
        setEditor({ id: crypto.randomUUID(), title: "", message: "" });
      }}><Plus aria-hidden="true" />{t("ownerNotices.create")}</button>}
    </header>
    <div className="owner-notices-surface">
      <div className="owner-notices-toolbar"><span><Bell aria-hidden="true" />{t("ownerNotices.allUsers")}</span><span className="owner-notices-state">{t("ownerNotices.drafts")}</span></div>
      <p className="owner-notices-boundary" role="note">{t("ownerNotices.previewOnly")}</p>
      {editor ? <form className="owner-notice-editor" aria-label={t("ownerNotices.editor")} onSubmit={event => {
        event.preventDefault();
        const next = { ...editor, title: editor.title.trim(), message: editor.message.trim() };
        if (!next.title || !next.message) return;
        setDrafts(current => current.some(item => item.id === next.id) ? current.map(item => item.id === next.id ? next : item) : [next, ...current]);
        finishEditor();
      }}>
        <label htmlFor="owner-notice-title">{t("ownerNotices.title")}</label>
        <input id="owner-notice-title" autoFocus required maxLength={120} value={editor.title} onChange={event => setEditor({ ...editor, title: event.target.value })} />
        <label htmlFor="owner-notice-message">{t("ownerNotices.message")}</label>
        <textarea id="owner-notice-message" required maxLength={4000} rows={7} value={editor.message} onChange={event => setEditor({ ...editor, message: event.target.value })} />
        <div className="owner-notice-editor-footer">
          <p id="owner-notice-delivery">{t("ownerNotices.deliveryUnavailable")}</p>
          <div className="owner-notice-actions">
            <button type="button" className="relay-button" onClick={finishEditor}>{t("common.cancel")}</button>
            <button type="submit" className="relay-button" disabled={!editor.title.trim() || !editor.message.trim()}>{t("ownerNotices.saveDraft")}</button>
            <button type="button" className="relay-button primary" disabled aria-describedby="owner-notice-delivery">{t("ownerNotices.send")}</button>
          </div>
        </div>
      </form> : drafts.length === 0 ? <div className="owner-notices-empty"><Bell aria-hidden="true" /><h2>{t("ownerNotices.empty")}</h2></div> : <ul className="owner-notice-list" aria-label={t("ownerNotices.drafts")}>
        {drafts.map(draft => <li key={draft.id}>
          <article className="owner-notice-row" aria-label={draft.title}>
            <div className="owner-notice-copy"><h2>{draft.title}</h2><p>{draft.message}</p><span className="owner-notice-draft-label">{t("ownerNotices.draft")}</span></div>
            <div className="owner-notice-actions">
              {deleting === draft.id ? <><span className="owner-notice-delete-question">{t("ownerNotices.deleteQuestion")}</span><button type="button" className="relay-button" onClick={() => { setDeleting(null); requestAnimationFrame(() => editButtons.current.get(draft.id)?.focus()); }}>{t("common.cancel")}</button><button type="button" className="relay-button owner-notice-delete" onClick={() => { setDrafts(current => current.filter(item => item.id !== draft.id)); setDeleting(null); requestAnimationFrame(() => createButton.current?.focus()); }}>{t("ownerNotices.delete")}</button></> : <><button ref={node => { if (node) editButtons.current.set(draft.id, node); else editButtons.current.delete(draft.id); }} type="button" className="relay-button" onClick={() => { returnTo.current = draft.id; setEditor({ ...draft }); }}>{t("common.edit")}</button><button type="button" className="relay-button" onClick={() => setDeleting(draft.id)}>{t("ownerNotices.delete")}</button></>}
            </div>
          </article>
        </li>)}
      </ul>}
    </div>
  </section>;
}
