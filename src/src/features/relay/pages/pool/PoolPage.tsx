import { useEffect, useState } from "react";
import { ArrowRightLeft, Download, Play, Plus, Power, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ConfigurationPresetPreview } from "../../api/types";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { Button, Dialog, EmptyState, PageHeader, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { SourceDialog } from "../connections/SourceDialog";
import { AddMembersDialog } from "./AddMembersDialog";
import { ModelRulesView } from "./ModelRules";
import { PoolMembersView } from "./PoolMembersView";
import { RoutingPolicyDialog } from "./RoutingPolicyDialog";

type View = "members" | "models";
export function PoolPage() {
  const { t } = useTranslation();
  const { mode, runtime, activateCodexProfile, busy, perform, codexPoolOauthSelection } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [routingPolicy, setRoutingPolicy] = useState(false);
  const [configurationPreview, setConfigurationPreview] = useState<ConfigurationPresetPreview | null>(null);
  const supportsModels = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("models"));
  const supportsMembers = mode !== "remote" || Boolean(runtime?.capabilities.features.some((feature) => feature === "accounts" || feature === "sources"));
  const supportsRoutingSettings = Boolean(runtime);
  const supportsConfigurationPresets = mode === "remote" && Boolean(runtime?.capabilities.features.includes("configuration_presets"));
  const canSaveConfigurationPreset = mode === "local" || supportsConfigurationPresets;
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
    const preview = await relayCommands.previewRemoteConfigurationPreset();
    if (preview) setConfigurationPreview(preview);
  });
  const poolToggleLabel = running ? t("pool.stop") : t("pool.start");
  const poolToggleShortLabel = running ? t("pool.stopShort") : t("pool.startShort");
  const action = <div className="pool-header-actions">
    {canSaveConfigurationPreset ? <div className="pool-preset-actions">
      <Button variant="secondary" icon={<Download aria-hidden />} aria-label={t("pool.exportConfiguration")} title={t("pool.exportConfiguration")} disabled={Boolean(busy)} busy={busy === "configuration-preset-export"} onClick={() => void exportConfiguration()}>{t("pool.exportConfigurationShort")}</Button>
      {supportsConfigurationPresets ? <Button variant="secondary" icon={<Upload aria-hidden />} aria-label={t("pool.importConfiguration")} title={t("pool.importConfiguration")} disabled={Boolean(busy)} busy={busy === "configuration-preset-preview"} onClick={() => void previewConfiguration()}>{t("pool.importConfigurationShort")}</Button> : null}
    </div> : null}
    {view === "members" ? <Button data-action="pool-add" variant="secondary" icon={<Plus aria-hidden />} aria-label={t("pool.addMember")} disabled={!supportsMembers} title={!supportsMembers ? t("remote.capabilityUnavailable") : t("pool.addMember")} onClick={() => setAddMembers(true)}>{t("pool.addMemberShort")}</Button> : null}
    {mode === "local" ? <>
      <Button data-action="pool-toggle" variant="secondary" icon={running ? <Power aria-hidden /> : <Play aria-hidden />} aria-label={poolToggleLabel} busy={busy === "pool-toggle"} title={poolToggleLabel} onClick={() => void perform("pool-toggle", running ? relayCommands.stopGateway : relayCommands.startGateway, running ? "feedback.stopped" : "feedback.started")}>{poolToggleShortLabel}</Button>
      <Button data-action="pool-switch" variant="primary" icon={<ArrowRightLeft aria-hidden />} aria-label={t("pool.switchChatGPT")} busy={busy === "pool-switch"} disabled={!running} title={!running ? t("pool.start") : t("pool.switchChatGPT")} onClick={() => void switchCodexToPool()}>{t("pool.switchChatGPTShort")}</Button>
    </> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(supportsModels ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page" data-view={view}><PageHeader title={t("nav.pool")} subtitle={t("pool.subtitle")} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("pool.views")} items={tabs} />{view === "members" ? <PoolMembersView onAdd={() => setAddMembers(true)} onRoutingPolicy={() => setRoutingPolicy(true)} supportsRoutingSettings={supportsRoutingSettings} /> : null}{view === "models" ? <ModelRulesView /> : null}{addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}{createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}{routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}{configurationPreview ? <ConfigurationPresetDialog preview={configurationPreview} onClose={() => setConfigurationPreview(null)} /> : null}{!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}</section>;
}

function ConfigurationPresetDialog({ preview, onClose }: { preview: ConfigurationPresetPreview; onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const apply = async () => {
    if (!preview.changes.length) return onClose();
    if (await perform("configuration-preset-apply", () => relayCommands.applyRemoteConfigurationPreset(preview), "feedback.saved")) onClose();
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
