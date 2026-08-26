import { useEffect, useState } from "react";
import { ArrowRightLeft, Download, Loader2, Plus, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, ConfigurationPresetPreview } from "../../api/types";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { Button, Dialog, EmptyState, IconButton, PageHeader, Tabs } from "../../components/Ui";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useRelayState } from "../../state/RelayStateProvider";
import { OAuthDialog } from "../connections/OAuthDialogs";
import { SourceDialog } from "../connections/SourceDialog";
import { AddMembersDialog } from "./AddMembersDialog";
import { ModelRulesView } from "./ModelRules";
import { PoolMembersView } from "./PoolMembersView";
import { RoutingPolicyDialog } from "./RoutingPolicyDialog";

type View = "members" | "models";
export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, activateCodexProfile, busy, perform, refresh, codexPoolOauthSelection } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [routingPolicy, setRoutingPolicy] = useState(false);
  const [configurationPreview, setConfigurationPreview] = useState<ConfigurationPresetPreview | null>(null);
  const oauth = useOAuthSignIn(() => refresh());
  const supportsModels = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("models"));
  const supportsMembers = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const supportsRoutingSettings = Boolean(runtime);
  const supportsConfigurationPresets = mode === "local" || Boolean(runtime?.capabilities.features.includes("configuration_presets"));
  const canSaveConfigurationPreset = mode === "local" || supportsConfigurationPresets;
  const reauthenticateAccount = (account: AccountSummary) => void oauth.start(false, account.id);
  useEffect(() => {
    if (view === "models" && !supportsModels) setView("members");
  }, [view, supportsModels]);
  const selectedOauthAccountId = codexPoolOauthSelection !== "none" && codexPoolOauthSelection !== "auto"
    && runtime?.accounts.some((account) => account.id === codexPoolOauthSelection && isCodexOauthAccountEligible(account))
    ? codexPoolOauthSelection
    : null;
  const switchCodexToPool = () => activateCodexProfile(
    "pool-switch",
    () => relayCommands.attachCodexGateway(selectedOauthAccountId, codexPoolOauthSelection === "none"),
    true,
  );
  const running = Boolean(runtime?.gateway.running);
  const exportConfiguration = () => perform("configuration-preset-export", mode === "local" ? relayCommands.exportLocalConfigurationPreset : relayCommands.exportRemoteConfigurationPreset);
  const previewConfiguration = () => perform("configuration-preset-preview", async () => {
    const preview = mode === "local"
      ? await relayCommands.previewLocalConfigurationPreset()
      : await relayCommands.previewRemoteConfigurationPreset();
    if (preview) setConfigurationPreview(preview);
  });
  const action = <div className="pool-header-actions">
    {canSaveConfigurationPreset ? <div className="pool-preset-actions">
      <IconButton className="pool-header-icon" label={t("pool.exportConfiguration")} icon={busy === "configuration-preset-export" ? <Loader2 className="spin" aria-hidden /> : <Download aria-hidden />} disabled={Boolean(busy)} title={t("pool.exportConfiguration")} onClick={() => void exportConfiguration()} />
      {supportsConfigurationPresets ? <IconButton className="pool-header-icon" label={t("pool.importConfiguration")} icon={busy === "configuration-preset-preview" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />} disabled={Boolean(busy)} title={t("pool.importConfiguration")} onClick={() => void previewConfiguration()} /> : null}
    </div> : null}
    {view === "members" ? <IconButton data-action="pool-add" className="pool-header-icon" label={t("pool.addMember")} icon={<Plus aria-hidden />} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : t("pool.addMember")} onClick={() => setAddMembers(true)} /> : null}
    {mode === "local" ? <Button data-action="pool-switch" variant="primary" icon={<ArrowRightLeft aria-hidden />} aria-label={t("pool.switchChatGPT")} busy={busy === "pool-switch"} disabled={!running} title={!running ? t("pool.start") : t("pool.switchChatGPT")} onClick={() => void switchCodexToPool()}>{t("pool.switchChatGPTShort")}</Button> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <PoolMembersView onAdd={() => setAddMembers(true)} onRoutingPolicy={() => setRoutingPolicy(true)} onReauthenticate={reauthenticateAccount} supportsRoutingSettings={supportsRoutingSettings} /> : null}{view === "models" ? <ModelRulesView /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{configurationPreview ? <ConfigurationPresetDialog preview={configurationPreview} mode={mode === "remote" ? "remote" : "local"} onClose={() => setConfigurationPreview(null)} /> : null}{oauth.flow ? <OAuthDialog flow={oauth.flow} onCancel={oauth.cancel} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function ConfigurationPresetDialog({ preview, mode, onClose }: { preview: ConfigurationPresetPreview; mode: "local" | "remote"; onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const apply = async () => {
    if (!preview.changes.length) return onClose();
    const applyPreset = mode === "local" ? relayCommands.applyLocalConfigurationPreset : relayCommands.applyRemoteConfigurationPreset;
    if (await perform("configuration-preset-apply", () => applyPreset(preview), "feedback.saved")) onClose();
  };
  return <Dialog wide title={t("pool.configurationPreset")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" icon={<Upload aria-hidden />} busy={busy === "configuration-preset-apply"} disabled={!preview.changes.length} onClick={() => void apply()}>{t("pool.applyConfiguration")}</Button></>}>
    <div className="configuration-preset-preview">
      <header><strong>{t("pool.configurationChanges", { count: preview.changes.length })}</strong><code title={preview.baseRevision}>{preview.baseRevision.slice(0, 16)}</code></header>
      {preview.changes.length ? <div className="table-wrap"><table><thead><tr><th>{t("pool.configurationSetting")}</th><th>{t("pool.configurationCurrent")}</th><th>{t("pool.configurationNext")}</th></tr></thead><tbody>{preview.changes.map((change) => <tr key={change.path}><th scope="row"><code>{formatConfigurationPath(change.path)}</code></th><td><code>{formatConfigurationValue(change.before)}</code></td><td><code>{formatConfigurationValue(change.after)}</code></td></tr>)}</tbody></table></div> : <EmptyState title={t("pool.configurationUnchanged")} description={t("pool.configurationUnchangedHint")} />}
    </div>
  </Dialog>;
}

function formatConfigurationPath(path: string) {
  return path.split("/").filter(Boolean).join(" / ");
}

function formatConfigurationValue(value: unknown) {
  if (typeof value === "string") return value;
  return JSON.stringify(value) ?? String(value);
}
