import { useId, useState } from "react";
import { groupModels } from "../../../../src/src/features/relay/modelGroups";
import { Dialog } from "../../components/Dialog";
import { ChevronDown, Pause, Search, Wallet } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { ownerMemberModels, ownerMemberModelPolicy, parseMemberPurchaseCost } from "./poolMemberSettings";
import type { ConnectionAction, OwnerConnection } from "./model";
import "./owner-pool-member-editor.css";

/** Account editor from Desktop PoolMemberEditor; no native commands or server writes. */
export function OwnerPoolMemberEditor({ member, change, onClose }: {
  member: OwnerConnection; change: (action: ConnectionAction) => void; onClose: () => void;
}) {
  const { t } = useLocale();
  const id = useId();
  const [tab, setTab] = useState("models");
  const [search, setSearch] = useState("");
  const { modelIds, enabledModels: initialEnabled } = ownerMemberModels(member);
  const [enabledModels, setEnabledModels] = useState(initialEnabled);
  const [paused, setPaused] = useState(member.paused);
  const [cost, setCost] = useState(member.account?.purchaseCostMicroUsd == null ? "" : String(member.account.purchaseCostMicroUsd / 1_000_000));
  const parsedCost = parseMemberPurchaseCost(cost);
  const groups = groupModels(modelIds.filter(model => model.toLowerCase().includes(search.trim().toLowerCase())), { isNativeChatGpt: () => true });
  const tabs = [{ id: "models", label: t("pool.memberModels") }, { id: "settings", label: t("pool.memberSettings") }];
  const save = () => {
    if (parsedCost === undefined) return;
    change({ type: "saveMember", id: member.id, settings: { ...ownerMemberModelPolicy(member, enabledModels), paused, purchaseCostMicroUsd: parsedCost } });
    onClose();
  };
  return <Dialog title={t("pool.editMember")} className="owner-member-dialog" onClose={onClose}>
    <div className="owner-member-identity"><span title={member.name}>{member.name}</span><small>{t("common.account")}</small></div>
    <div className="owner-member-tabs relay-tabs" role="tablist" aria-label={t("pool.editMember")} onKeyDown={event => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const next = event.key === "Home" ? 0 : event.key === "End" ? 1 : tab === "models" ? 1 : 0;
      setTab(tabs[next]!.id);
      event.currentTarget.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
    }}>
      {tabs.map(item => <button key={item.id} id={`${id}-${item.id}`} role="tab" type="button" className={tab === item.id ? "active" : ""} aria-selected={tab === item.id} aria-controls={`${id}-panel`} tabIndex={tab === item.id ? 0 : -1} data-initial-focus={item.id === "models" ? "" : undefined} onClick={() => setTab(item.id)}>{item.label}</button>)}
    </div>
    <div className="owner-member-panel" id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${tab}`} tabIndex={0}>
      {tab === "models" ? <section className="owner-member-models">
        <div className="owner-member-model-heading"><strong>{t("pool.allowedModels")}</strong><span>{enabledModels.length} / {modelIds.length}</span></div>
        {modelIds.length > 6 && <label className="owner-member-search"><Search aria-hidden="true" /><input type="search" aria-label={t("pool.searchMemberModels")} placeholder={t("pool.searchMemberModels")} value={search} onChange={event => setSearch(event.target.value)} /></label>}
        {groups.length ? groups.map(group => <details className="owner-member-model-group" key={`${group.id}:${Boolean(search.trim())}`} open>
          <summary><strong>{group.label}</strong><span>{group.models.filter(model => enabledModels.includes(model)).length} / {group.models.length}</span><ChevronDown aria-hidden="true" /></summary>
          <ul>{group.models.map(model => <li key={model} data-member-model-id={model} data-enabled={enabledModels.includes(model)}>
            <label><code>{model}</code><input className="owner-member-switch" type="checkbox" role="switch" aria-label={t("pool.allowMemberModel", { model })} checked={enabledModels.includes(model)} onChange={() => setEnabledModels(current => current.includes(model) ? current.filter(value => value !== model) : [...current, model])} /></label>
          </li>)}</ul>
        </details>) : <p className="owner-member-empty" role="status">{t(modelIds.length ? "pool.noMatchingModels" : "pool.noMemberModels")}</p>}
      </section> : <div className="owner-member-settings">
        <label className="owner-member-setting" data-member-setting="pause"><span className="owner-member-setting-label"><Pause aria-hidden="true" /><span>{t("pool.pauseMember")}</span></span><input className="owner-member-switch" type="checkbox" role="switch" aria-label={t("pool.pauseMember")} checked={paused} onChange={event => setPaused(event.target.checked)} /></label>
        <div className="owner-member-setting" data-member-setting="cost"><label className="owner-member-setting-label" htmlFor={`${id}-cost`}><Wallet aria-hidden="true" /><span>{t("pool.purchaseCost")}</span></label><span className="owner-member-cost"><span aria-hidden="true">$</span><input id={`${id}-cost`} type="text" inputMode="decimal" autoComplete="off" spellCheck={false} aria-invalid={parsedCost === undefined || undefined} aria-describedby={parsedCost === undefined ? `${id}-cost-error` : undefined} value={cost} onChange={event => setCost(event.target.value)} placeholder={t("pool.purchaseCostNotSet")} /></span></div>
        {parsedCost === undefined && <p className="owner-member-error" id={`${id}-cost-error`} role="alert">{t("pool.invalidPurchaseCost")}</p>}
      </div>}
    </div>
    <footer className="owner-member-footer"><button className="relay-button" type="button" onClick={onClose}>{t("common.cancel")}</button><button className="relay-button primary" type="button" disabled={parsedCost === undefined} onClick={save}>{t("common.save")}</button></footer>
  </Dialog>;
}
