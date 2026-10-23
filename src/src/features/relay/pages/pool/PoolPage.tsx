import { useEffect, useState } from "react";
import { Download, Loader2, Plug, Plus, RotateCcw, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, ConfigurationPresetPreview } from "../../api/types";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { ActionMenu, ActionMenuItem, Button, Dialog, EmptyState, IconButton, PageHeader, Tabs } from "../../components/Ui";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useClaudeDesktopLaunch } from "../../hooks/useClaudeDesktopLaunch";
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
  const { mode, runtime, activateCodexProfile, busy, perform, refresh, codexPoolOauthSelection, launchApplicationAfterConnect } = useRelayState();
  const [view, setView] = useState<View>("members");
  const [createSource, setCreateSource] = useState(false);
  const [addMembers, setAddMembers] = useState(false);
  const [connectDialog, setConnectDialog] = useState(false);
  const [routingPolicy, setRoutingPolicy] = useState(false);
  const [configurationPreview, setConfigurationPreview] = useState<ConfigurationPresetPreview | null>(null);
  const oauth = useOAuthSignIn(() => refresh());
  const claudeDesktop = useClaudeDesktopLaunch();
  const localPool = mode === "local";
  const supportsRoutingSettings = localPool && Boolean(runtime);
  const reauthenticateAccount = (account: AccountSummary, automatic = false) => void oauth.start(automatic, account.id, undefined, undefined, automatic);
  useEffect(() => {
    if (view === "models" && !localPool) setView("members");
  }, [view, localPool]);
  const selectedOauthAccountId = codexPoolOauthSelection !== "none"
    && runtime?.accounts.some((account) => account.id === codexPoolOauthSelection && isCodexOauthAccountEligible(account))
    ? codexPoolOauthSelection
    : null;
  const switchCodexToPool = () => activateCodexProfile(
    "pool-switch",
    () => relayCommands.attachCodexGateway(selectedOauthAccountId, selectedOauthAccountId === null),
    launchApplicationAfterConnect,
  );
  const connectOpenCode = () => perform("opencode-connect", relayCommands.connectOpenCode, launchApplicationAfterConnect ? "feedback.launched" : "feedback.saved", {
    backgroundRefresh: true,
    ...(launchApplicationAfterConnect ? { afterWork: relayCommands.restartOpenCode } : {}),
  });
  const connectClaudeDesktop = () => perform("claude-desktop-connect", relayCommands.connectClaudeDesktop, launchApplicationAfterConnect ? "feedback.launched" : "feedback.saved", {
    backgroundRefresh: true,
    ...(launchApplicationAfterConnect ? { afterWork: claudeDesktop.launch } : {}),
  });
  const running = Boolean(runtime?.gateway.running);
  const exportConfiguration = () => perform("configuration-preset-export", relayCommands.exportLocalConfigurationPreset, undefined, { backgroundRefresh: true });
  const previewConfiguration = () => perform("configuration-preset-preview", async () => {
    const preview = await relayCommands.previewLocalConfigurationPreset();
    if (preview) setConfigurationPreview(preview);
  }, undefined, { backgroundRefresh: true });
  const action = <div className="pool-header-actions">
    {view === "members" ? (
      <Button
        data-action="pool-add"
        aria-label={t("pool.addMember")}
        icon={<Plus aria-hidden />}
        disabled={!localPool}
        title={!localPool ? t("common.unsupported") : undefined}
        onClick={() => setAddMembers(true)}
      >
        {t("pool.addMemberShort")}
      </Button>
    ) : null}
    {view === "models" && localPool ? (
      <IconButton
        className="pool-header-icon"
        label={t("models.resetOrder")}
        icon={busy === "model-order-reset" ? <Loader2 className="spin" aria-hidden /> : <RotateCcw aria-hidden />}
        disabled={busy === "model-order" || busy === "model-order-reset" || !runtime?.gateway.models?.length}
        onClick={() => void perform("model-order-reset", () => relayCommands.setModelDisplayOrder([]), "feedback.saved", { backgroundRefresh: true })}
      />
    ) : null}
    {localPool ? (
      <Button
        data-action="pool-connect"
        variant="primary"
        icon={<Plug aria-hidden />}
        aria-label={t("pool.connect")}
        busy={busy === "pool-switch" || busy === "opencode-connect" || busy === "claude-desktop-connect"}
        disabled={!running}
        title={!running ? t("pool.start") : undefined}
        onClick={() => setConnectDialog(true)}
      >
        {t("pool.connect")}
      </Button>
    ) : null}
    {localPool ? <ActionMenu className="pool-preset-menu" label={t("pool.configurationPreset")}>
      <ActionMenuItem
        icon={busy === "configuration-preset-export" ? <Loader2 className="spin" aria-hidden /> : <Download aria-hidden />}
        disabled={busy === "configuration-preset-export"}
        onClick={() => void exportConfiguration()}
      >
        {t("pool.exportConfiguration")}
      </ActionMenuItem>
      <ActionMenuItem
        icon={busy === "configuration-preset-preview" ? <Loader2 className="spin" aria-hidden /> : <Upload aria-hidden />}
        disabled={busy === "configuration-preset-preview"}
        onClick={() => void previewConfiguration()}
      >
        {t("pool.importConfiguration")}
      </ActionMenuItem>
    </ActionMenu> : null}
  </div>;
  const tabs = [{ id: "members", label: t("pool.members") }, ...(localPool ? [{ id: "models", label: t("pool.modelRules") }] : [])];
  return <section className="relay-page relay-workspace-page" data-view={view}>
    <PageHeader title={t("nav.pool")} actions={action} navigation={<Tabs value={view} onChange={(viewId) => setView(viewId as View)} label={t("pool.views")} items={tabs} />} />
    {view === "members" ? (
      <PoolMembersView
        onAdd={() => setAddMembers(true)}
        onRoutingPolicy={() => setRoutingPolicy(true)}
        onReauthenticate={reauthenticateAccount}
        supportsRoutingSettings={supportsRoutingSettings}
      />
    ) : null}
    {view === "models" ? <ModelRulesView /> : null}
    {addMembers ? <AddMembersDialog onClose={() => setAddMembers(false)} onAddSource={() => { setAddMembers(false); setCreateSource(true); }} /> : null}
    {connectDialog ? (
      <ApplicationPickerDialog
        onClose={() => setConnectDialog(false)}
        onChatGPT={() => void switchCodexToPool()}
        onOpenCode={() => void connectOpenCode()}
        onClaudeDesktop={() => void connectClaudeDesktop()}
      />
    ) : null}
    {createSource ? <SourceDialog source={null} addToPool onClose={() => setCreateSource(false)} /> : null}
    {routingPolicy ? <RoutingPolicyDialog onClose={() => setRoutingPolicy(false)} /> : null}
    {configurationPreview ? (
      <ConfigurationPresetDialog
        preview={configurationPreview}
        onClose={() => setConfigurationPreview(null)}
      />
    ) : null}
    {oauth.flow ? <OAuthDialog flow={oauth.flow} starting={oauth.starting} onCancel={oauth.cancel} /> : null}
    {!runtime ? <span className="sr-only">{t("common.notConfigured")}</span> : null}
  </section>;
}

function ConfigurationPresetDialog({ preview, onClose }: { preview: ConfigurationPresetPreview; onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const apply = async () => {
    if (!preview.changes.length) return onClose();
    const applyPreset = relayCommands.applyLocalConfigurationPreset;
    if (await perform("configuration-preset-apply", () => applyPreset(preview), "feedback.saved", { backgroundRefresh: true })) onClose();
  };
  return (
    <Dialog
      wide
      title={t("pool.configurationPreset")}
      onClose={onClose}
      footer={(
        <>
          <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
          <Button
            variant="primary"
            icon={<Upload aria-hidden />}
            busy={busy === "configuration-preset-apply"}
            disabled={!preview.changes.length}
            onClick={() => void apply()}
          >
            {t("pool.applyConfiguration")}
          </Button>
        </>
      )}
    >
      <div className="configuration-preset-preview">
        <header>
          <strong>{t("pool.configurationChanges", { count: preview.changes.length })}</strong>
          <code data-relay-tooltip={preview.baseRevision}>{preview.baseRevision.slice(0, 16)}</code>
        </header>
        {preview.changes.length ? (
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>{t("pool.configurationSetting")}</th>
                  <th>{t("pool.configurationCurrent")}</th>
                  <th>{t("pool.configurationNext")}</th>
                </tr>
              </thead>
              <tbody>
                {preview.changes.map((change) => (
                  <tr key={change.path}>
                    <th scope="row"><code>{formatConfigurationPath(change.path)}</code></th>
                    <td><code>{formatConfigurationValue(change.before)}</code></td>
                    <td><code>{formatConfigurationValue(change.after)}</code></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <EmptyState title={t("pool.configurationUnchanged")} />
        )}
      </div>
    </Dialog>
  );
}

function formatConfigurationPath(path: string) {
  return path.split("/").filter(Boolean).join(" / ");
}

function formatConfigurationValue(configurationValue: unknown) {
  if (typeof configurationValue === "string") return configurationValue;
  return JSON.stringify(configurationValue) ?? String(configurationValue);
}
