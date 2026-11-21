import { useId, useState } from "react";
import type { Rule } from "../../api/catalog";
import { Dialog } from "../../components/Dialog";
import { Search } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";

export function PoolModelPicker({ candidates, blocked, close, admit }: {
  candidates: Rule[]; blocked: boolean; close: () => void; admit: (rule: Rule) => void;
}) {
  const { t, locale } = useLocale();
  const [query, setQuery] = useState("");
  const [choice, setChoice] = useState<string>();
  const name = useId();
  const search = query.trim().toLocaleLowerCase(locale);
  const visible = candidates.filter(row => [row.source, row.model].some(v => v.toLocaleLowerCase(locale).includes(search)));
  const selected = visible.find(row => row.id === choice);
  const captions = new Map<string, number>();
  for (const row of candidates) {
    const key = `${row.source}\n${row.model}`;
    captions.set(key, (captions.get(key) ?? 0) + 1);
  }
  return <Dialog className="owner-source-dialog live-pool-picker" title={t("pool.addMember")} onClose={close}>
    {!candidates.length ? <>
      <p>{t("pool.noQualifiedModels")}</p>
      <a className="entry-account-link" href="/admin/connections">{t("nav.connections")}</a>
    </> : <form onSubmit={event => { event.preventDefault(); if (!blocked && selected) admit(selected); }}>
      {candidates.length > 6 && <label className="owner-resource-search"><Search aria-hidden="true" />
        <input type="search" data-initial-focus aria-label={t("pool.findQualifiedModel")} placeholder={t("pool.findQualifiedModel")}
          value={query} onChange={event => setQuery(event.currentTarget.value)} />
      </label>}
      <fieldset className="live-pool-options" disabled={blocked}>
        <legend className="sr-only">{t("pool.memberModels")}</legend>
        {visible.map((row, index) => <label key={row.id} className="live-pool-option">
          <span><strong>{row.source}</strong><code>{row.model}</code>
            {(captions.get(`${row.source}\n${row.model}`) ?? 0) > 1 && <small><code>{row.id}</code></small>}
          </span>
          <input name={name} type="radio" data-initial-focus={(candidates.length <= 6 && index === 0) || undefined}
            checked={choice === row.id} onChange={() => setChoice(row.id)} />
        </label>)}
        {!visible.length && <p role="status">{t("pool.noMatchingModels")}</p>}
      </fieldset>
      <div className="button-row">
        <button type="button" className="key-button key-secondary" onClick={close}>{t("common.cancel")}</button>
        <button type="submit" className="key-button key-primary" disabled={blocked || !selected}>{t("connections.addToPool")}</button>
      </div>
    </form>}
  </Dialog>;
}
