import { useEffect, useState } from "react";
import { KeyRound, Plus, RotateCcw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, KeySummary, SourceSummary } from "../../api/types";
import { Button, Dialog, EmptyState, PageHeader, QuotaMeter, StatusBadge, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type View = "members" | "keys" | "models";
type Member = (AccountSummary & { kind: "account" }) | (SourceSummary & { kind: "source"; health: string; quota: null });

export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, setPage } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createKey, setCreateKey] = useState(false);
  const supportsKeys = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("keys"));
  const supportsModels = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("models"));
  const supportsMembers = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  useEffect(() => {
    if ((view === "keys" && !supportsKeys) || (view === "models" && !supportsModels)) setView("members");
  }, [view, supportsKeys, supportsModels]);
  const action = view === "keys"
    ? <Button variant="primary" icon={<KeyRound aria-hidden />} disabled={!supportsKeys} title={!supportsKeys ? t("remote.capabilityUnavailable") : undefined} onClick={() => setCreateKey(true)}>{t("keys.create")}</Button>
    : view === "members"
      ? <Button variant="primary" icon={<Plus aria-hidden />} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : undefined} onClick={() => setPage("connections")}>{t("pool.addMember")}</Button>
      : null;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsKeys ? [{ id: "keys", label: t("pool.keys") }] : []), ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page"><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <MembersView /> : null}{view === "keys" ? <KeysView onCreate={() => setCreateKey(true)} /> : null}{view === "models" ? <ModelsView /> : null}{createKey ? <CreateKeyDialog onClose={() => setCreateKey(false)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function MembersView() {
  const { t } = useTranslation();
  const { mode, runtime, setPage } = useRelayState();
  const canAdd = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const members: Member[] = [
    ...(runtime?.accounts ?? []).map((item) => ({ ...item, kind: "account" as const })),
    ...(runtime?.sources ?? []).map((item) => ({ ...item, kind: "source" as const, health: item.enabled ? "healthy" : "disabled", quota: null })),
  ];
  const selected = members.find((member) => `${member.kind}:${member.id}` === selectedId) ?? null;
  if (!members.length) return <EmptyState title={t("pool.emptyTitle")} description={t("pool.emptyDescription")} action={<Button variant="primary" disabled={!canAdd} title={!canAdd ? t("remote.capabilityUnavailable") : undefined} onClick={() => setPage("connections")}>{t("pool.addMember")}</Button>} />;
  const counts = { healthy: members.filter((item) => item.enabled && item.health === "healthy").length, limited: members.filter((item) => item.kind === "account" && item.quota.primary?.availableBasisPoints === 0).length, disabled: members.filter((item) => !item.enabled).length };
  return <><div className="pool-summary"><div><span>{t("pool.healthy")}</span><strong>{counts.healthy}</strong></div><div><span>{t("pool.limited")}</span><strong>{counts.limited}</strong></div><div><span>{t("common.disabled")}</span><strong>{counts.disabled}</strong></div><div><span>{t("common.models")}</span><strong>{runtime?.gateway.visibleModelIds.length ?? 0}</strong></div></div><div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("common.status")}</th><th>{t("common.type")}</th><th>{t("common.name")}</th><th>{t("common.health")}</th><th>{t("common.quota")}</th><th>{t("pool.priority")}</th><th>{t("pool.weight")}</th></tr></thead><tbody>{members.map((member) => <tr key={`${member.kind}-${member.id}`} className={selectedId === `${member.kind}:${member.id}` ? "selected" : ""}><td><StatusBadge status={member.enabled ? "ready" : "disabled"} label={member.enabled ? t("common.enabled") : t("common.disabled")} /></td><td>{t(`pool.types.${member.kind}`)}</td><td><button type="button" className="request-link" onClick={() => setSelectedId(`${member.kind}:${member.id}`)}><strong>{member.kind === "source" ? member.name : member.label}</strong></button></td><td>{t(`health.${member.health}`, { defaultValue: member.health })}</td><td>{member.kind === "account" ? <QuotaMeter window={member.quota.primary} label={t("quota.primary")} /> : t("common.unsupported")}</td><td>{member.priority}</td><td>{member.weight}</td></tr>)}</tbody></table></div>{selected ? <MemberEditor key={`${selected.kind}:${selected.id}`} member={selected} onClose={() => setSelectedId(null)} /> : <p className="form-note">{t("pool.selectMemberHint")}</p>}</>;
}

function MemberEditor({ member, onClose }: { member: Member; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canSave = mode !== "remote" || Boolean(runtime?.capabilities.features.includes(member.kind === "account" ? "accounts" : "sources"));
  const [priority, setPriority] = useState(member.priority);
  const [weight, setWeight] = useState(member.weight);
  const [allowed, setAllowed] = useState(member.allowedModels.join(", "));
  const [excluded, setExcluded] = useState(member.excludedModels.join(", "));
  const [draining, setDraining] = useState(member.draining);
  const save = async () => {
    const allowedModels = parseList(allowed);
    const excludedModels = parseList(excluded);
    const payload = { priority, weight, allowedModels, excludedModels, draining };
    await perform(`member-${member.id}`, () => {
      if (member.kind === "account") return mode === "local"
        ? relayCommands.updateAccount({ accountId: member.id, priority, weight, allowedModels, excludedModels, draining })
        : relayCommands.remoteAction({ type: "update_account", id: member.id }, payload);
      const sourcePayload = { sourceId: member.id, name: member.name, baseUrl: member.baseUrl, wireApi: member.wireApi, models: member.models, allowedModels, excludedModels, draining, priority, weight };
      return mode === "local" ? relayCommands.updateSource(sourcePayload) : relayCommands.remoteAction({ type: "update_source", id: member.id }, payload);
    }, "feedback.saved");
  };
  return <section className="member-editor"><header><div><h2>{t("pool.editMember")}</h2><p>{member.kind === "source" ? member.name : member.label}</p></div><Button variant="ghost" onClick={onClose}>{t("common.close")}</Button></header><div className="settings-row"><label><span>{t("pool.priority")}</span><input type="number" value={priority} onChange={(event) => setPriority(Number(event.target.value))} /></label><label><span>{t("pool.weight")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label><label className="toggle-row"><input type="checkbox" checked={draining} onChange={(event) => setDraining(event.target.checked)} /><span>{t("accounts.drain")}</span></label></div><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} placeholder="gpt-5.4, gpt-5.4-mini" /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><footer><span className="form-note">{t("pool.modelListHint")}</span><Button variant="primary" busy={busy === `member-${member.id}`} disabled={!canSave} title={!canSave ? t("remote.capabilityUnavailable") : undefined} onClick={save}>{t("pool.savePolicy")}</Button></footer></section>;
}

function KeysView({ onCreate }: { onCreate: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("keys"));
  const [revealed, setRevealed] = useState("");
  const [editing, setEditing] = useState<KeySummary | null>(null);
  if (!runtime?.keys.length) return <EmptyState title={t("keys.emptyTitle")} description={t("keys.emptyDescription")} action={<Button variant="primary" disabled={!canManage} title={!canManage ? t("remote.capabilityUnavailable") : undefined} onClick={onCreate}>{t("keys.create")}</Button>} />;
  return <><div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("keys.masked")}</th><th>{t("keys.scope")}</th><th>{t("common.models")}</th><th>{t("common.lastUsed")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead><tbody>{runtime.keys.map((key) => <tr key={key.id}><td><StatusBadge status={key.enabled ? "ready" : "disabled"} label={key.enabled ? t("common.enabled") : t("common.disabled")} /></td><td>{key.label}</td><td><code>zlr_••••••••••••</code></td><td>{(key.accountIds?.length || key.sourceIds?.length) ? t("keys.scoped") : t("keys.allMembers")}</td><td>{key.allowedModels.length || t("keys.allModels")}</td><td>{key.lastUsedAtMs ? new Date(key.lastUsedAtMs).toLocaleString() : t("common.never")}</td><td className="row-actions"><Button variant="ghost" onClick={() => setEditing(key)}>{t("keys.editPolicy")}</Button><Button variant="ghost" onClick={() => perform(`enable-${key.id}`, () => mode === "local" ? relayCommands.setKeyEnabled(key.id, !key.enabled) : relayCommands.remoteAction({ type: "update_key", id: key.id }, { enabled: !key.enabled }), "feedback.saved")}>{key.enabled ? t("common.disable") : t("common.enable")}</Button><Button variant="ghost" busy={busy === `rotate-${key.id}`} icon={<RotateCcw aria-hidden />} onClick={async () => { if (!window.confirm(t("keys.rotateConfirm"))) return; const result: { current: { secret: string } | null } = { current: null }; await perform(`rotate-${key.id}`, async () => { result.current = mode === "local" ? await relayCommands.rotateKey(key.id) : await relayCommands.remoteAction({ type: "rotate_key", id: key.id }) as { secret: string }; }, "feedback.keyRotated"); if (result.current) setRevealed(result.current.secret); }}>{t("keys.rotate")}</Button><Button variant="ghost" icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("keys.deleteConfirm"))) void perform(`delete-${key.id}`, () => mode === "local" ? relayCommands.deleteKey(key.id) : relayCommands.remoteAction({ type: "delete_key", id: key.id }), "feedback.deleted"); }}>{t("common.delete")}</Button></td></tr>)}</tbody></table></div>{revealed ? <div className="one-time-secret" role="status"><strong>{t("keys.copyNow")}</strong><code>{revealed}</code><Button variant="secondary" onClick={() => navigator.clipboard.writeText(revealed)}>{t("common.copy")}</Button></div> : null}{editing ? <KeyPolicyDialog key={editing.id} value={editing} onClose={() => setEditing(null)} /> : null}</>;
}

function KeyPolicyDialog({ value, onClose }: { value: KeySummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [label, setLabel] = useState(value.label);
  const [sourceIds, setSourceIds] = useState(value.sourceIds ?? []);
  const [accountIds, setAccountIds] = useState(value.accountIds ?? []);
  const [allowed, setAllowed] = useState(value.allowedModels.join(", "));
  const [excluded, setExcluded] = useState(value.excludedModels.join(", "));
  const [prefix, setPrefix] = useState(value.modelPrefix ?? "");
  const save = async () => {
    const payload = { label, sourceIds: sourceIds.length ? sourceIds : null, accountIds: accountIds.length ? accountIds : null, allowedModels: parseList(allowed), excludedModels: parseList(excluded), modelPrefix: prefix.trim() || null };
    const ok = await perform(`key-policy-${value.id}`, () => mode === "local" ? relayCommands.updateKey({ keyId: value.id, ...payload }) : relayCommands.remoteAction({ type: "update_key", id: value.id }, payload), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog wide title={t("keys.editPolicy")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === `key-policy-${value.id}`} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("keys.label")}</span><input value={label} onChange={(event) => setLabel(event.target.value)} /></label><fieldset><legend>{t("pool.members")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => setAccountIds(toggle(accountIds, account.id))} />{account.label}</label>)}{runtime?.sources.map((source) => <label key={source.id}><input type="checkbox" checked={sourceIds.includes(source.id)} onChange={() => setSourceIds(toggle(sourceIds, source.id))} />{source.name}</label>)}</div></fieldset><label className="relay-field"><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label className="relay-field"><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label><label className="relay-field"><span>{t("keys.modelPrefix")}</span><input value={prefix} onChange={(event) => setPrefix(event.target.value)} /></label></div></Dialog>;
}

function ModelsView() {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  if (!runtime?.gateway.visibleModelIds.length) return <EmptyState title={t("models.emptyTitle")} description={t("models.emptyDescription")} />;
  return <section className="model-rules"><header><h2>{t("models.visible")}</h2><p>{t("models.explanation")}</p></header><ul>{runtime.gateway.visibleModelIds.map((model) => <li key={model}><code>{model}</code><StatusBadge status="ready" label={t("models.available")} /><span>{[...runtime.sources, ...runtime.accounts].filter((item) => item.models.includes(model)).length} {t("pool.members").toLowerCase()}</span></li>)}</ul></section>;
}

function CreateKeyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [label, setLabel] = useState("Codex");
  const [secret, setSecret] = useState("");
  useEffect(() => () => setSecret(""), []);
  const create = async () => { const result: { current: { secret: string } | null } = { current: null }; const ok = await perform("key-create", async () => { result.current = mode === "local" ? await relayCommands.createKey(label) : await relayCommands.remoteAction({ type: "create_key" }, { label, sourceIds: null, accountIds: null, allowedModels: [], excludedModels: [], modelPrefix: null }) as { secret: string }; }, "feedback.keyCreated"); if (ok && result.current) setSecret(result.current.secret); };
  return <Dialog title={t("keys.create")} onClose={onClose} footer={secret ? <Button variant="primary" onClick={onClose}>{t("common.done")}</Button> : <><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "key-create"} onClick={create}>{t("keys.create")}</Button></>}>{secret ? <div className="one-time-secret"><strong>{t("keys.copyNow")}</strong><code>{secret}</code><Button variant="secondary" onClick={() => navigator.clipboard.writeText(secret)}>{t("common.copy")}</Button><p>{t("keys.shownOnce")}</p></div> : <label className="relay-field"><span>{t("keys.label")}</span><input value={label} onChange={(event) => setLabel(event.target.value)} /></label>}</Dialog>;
}

function parseList(value: string) { return [...new Set(value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean))]; }
function toggle(values: string[], value: string) { return values.includes(value) ? values.filter((item) => item !== value) : [...values, value]; }
