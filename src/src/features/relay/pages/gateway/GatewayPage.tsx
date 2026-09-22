import { useEffect, useState } from "react";
import { ArrowRightLeft, CircleAlert, Play, RotateCw, Square, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { ActionMenu, ActionMenuItem, Button, EmptyState, OptionMenu, PageHeader, SettingToggle, Tabs, formatAccountPlan } from "../../components/Ui";
import { CodexBackgroundTasksControl } from "../../components/CodexBackgroundTasksControl";
import { CodexWebsocketsControl } from "../../components/CodexWebsocketsControl";
import { ChatgptRetryUntilAvailableControl } from "../../components/ChatgptRetryUntilAvailableControl";
import { useRelayState } from "../../state/RelayStateProvider";
import { GatewayApiTab } from "./GatewayApiTab";

type GatewayTab = "api" | "chatgpt" | "opencode";

export function GatewayPage() {
  const { t } = useTranslation();
  const { mode, runtime, readyState, busy, perform } = useRelayState();
  const [activeTab, setActiveTab] = useState<GatewayTab>("api");
  const running = mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running);
  const endpoint = mode === "zenith" ? "https://api.zenithmarket.dev/v1" : runtime?.gateway.baseUrl ?? "";
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("local_gateway"));
  const tabs: Array<{ id: GatewayTab; label: string }> = mode === "zenith"
    ? [{ id: "api", label: t("gateway.tabs.api") }]
    : [
      { id: "api", label: t("gateway.tabs.api") },
      { id: "chatgpt", label: t("gateway.tabs.chatgpt") },
      { id: "opencode", label: t("gateway.tabs.opencode") },
    ];

  useEffect(() => {
    if (mode === "zenith") setActiveTab("api");
  }, [mode]);

  const restart = () => perform("gateway-restart", async () => {
    if (mode === "local") {
      await relayCommands.restartGateway();
    } else {
      await relayCommands.remoteAction({ type: "stop_gateway" });
      await relayCommands.remoteAction({ type: "start_gateway" });
    }
  }, "feedback.restarted");

  const apiActions = mode === "zenith" ? null : <>
    <ActionMenu>
      <ActionMenuItem icon={<RotateCw aria-hidden />} disabled={!running || !canManage || busy === "gateway-restart"} onClick={restart}>
        {t("gateway.restart")}
      </ActionMenuItem>
    </ActionMenu>
    <Button
      variant={running ? "secondary" : "primary"}
      busy={busy === "gateway-toggle"}
      disabled={!canManage}
      title={!canManage ? t("common.unsupported") : undefined}
      icon={running ? <Square aria-hidden /> : <Play aria-hidden />}
      onClick={() => perform(
        "gateway-toggle",
        () => mode === "local"
          ? (running ? relayCommands.stopGateway() : relayCommands.startGateway())
          : relayCommands.remoteAction({ type: running ? "stop_gateway" : "start_gateway" }),
        running ? "feedback.stopped" : "feedback.started",
      )}
    >
      {running ? t("gateway.stop") : t("gateway.start")}
    </Button>
  </>;

  const chatGptActions = mode === "local" ? <Button
    variant="primary"
    busy={busy === "chatgpt-launch"}
    disabled={!running}
    title={!running ? t("gateway.start") : undefined}
    icon={<Play aria-hidden />}
    onClick={() => perform("chatgpt-launch", relayCommands.launchManagedCodex, "feedback.launched")}
  >
    {t("gateway.launchChatGPT")}
  </Button> : null;

  const openCodeActions = mode === "local" ? <Button
    variant="primary"
    busy={busy === "opencode-launch"}
    disabled={!running}
    title={!running ? t("gateway.start") : undefined}
    icon={<Play aria-hidden />}
    onClick={() => perform("opencode-launch", relayCommands.restartOpenCode, "feedback.launched")}
  >
    {t("gateway.launchOpenCode")}
  </Button> : null;

  return <section className="relay-page gateway-page">
    <PageHeader
      title={t("nav.gateway")}
      subtitle={t(`gateway.tabSubtitles.${activeTab}.${mode}`)}
      actions={activeTab === "api" ? apiActions : activeTab === "chatgpt" ? chatGptActions : openCodeActions}
    />
    <Tabs value={activeTab} onChange={(value) => setActiveTab(value as GatewayTab)} label={t("gateway.tabs.label")} items={tabs} />
    {activeTab === "api"
      ? <GatewayApiTab running={running} endpoint={endpoint} />
      : activeTab === "chatgpt" ? <GatewayChatGPTTab /> : <GatewayOpenCodeTab />}
  </section>;
}

function GatewayChatGPTTab() {
  const { t } = useTranslation();
  const { mode, runtime } = useRelayState();
  if (mode === "zenith") return <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />;
  const showSettings = mode === "local" || runtime?.capabilities.features.some((feature) =>
    feature === "codex_background_tasks" || feature === "codex_websockets" || feature === "chatgpt_retry_until_available",
  );
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.chatgpt")}>
    <div className="gateway-workspace">
      <ChatGPTSetup />
      {showSettings ? <div className="gateway-settings-panel gateway-application-panel">
        <CodexBackgroundTasksControl className="gateway-setting-row" />
        <CodexWebsocketsControl className="gateway-setting-row" />
        <ChatgptRetryUntilAvailableControl className="gateway-setting-row" />
      </div> : null}
    </div>
  </section>;
}

function GatewayOpenCodeTab() {
  const { t } = useTranslation();
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <div className="gateway-opencode-panel">
      <div className="gateway-opencode-development" role="status">
        <strong>{t("gateway.openCodeInDevelopment")}</strong>
      </div>
    </div>
  </section>;
}

function ChatGPTSetup() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, activateCodexProfile, codexPoolOauthSelection, setCodexPoolOauthSelection } = useRelayState();
  const eligibleAccounts = (runtime?.accounts ?? [])
    .filter(isCodexOauthAccountEligible)
    .sort((left, right) => left.label.localeCompare(right.label) || left.id.localeCompare(right.id));
  const eligibleAccountIds = eligibleAccounts.map((account) => account.id).join("\0");
  const reserveEnabled = (runtime?.gateway.chatgptInterfaceQuotaReserveBasisPoints ?? 100) > 0;

  useEffect(() => {
    if (!runtime || mode !== "local" || codexPoolOauthSelection === "none" || codexPoolOauthSelection === "auto") return;
    const ids = eligibleAccountIds ? eligibleAccountIds.split("\0") : [];
    if (!ids.includes(codexPoolOauthSelection)) setCodexPoolOauthSelection("auto");
  }, [codexPoolOauthSelection, eligibleAccountIds, mode, runtime, setCodexPoolOauthSelection]);

  if (mode === "remote") {
    const canAttach = Boolean(runtime?.capabilities.features.includes("profile_attach"));
    const switchRemote = () => activateCodexProfile("gateway-client-switch", relayCommands.attachCodexRemoteGateway, true);
    return <section className="gateway-account-panel client-setup codex-client-setup client-oauth-binding remote-client-setup">
      <header>
        <span className="gateway-config-icon"><UserRound aria-hidden /></span>
        <div><h2>{t("gateway.clientSetup")}</h2><p>{t("gateway.remoteClientHint")}</p></div>
      </header>
      <Button variant="secondary" icon={<ArrowRightLeft aria-hidden />} busy={busy === "gateway-client-switch"} disabled={!runtime?.gateway.running || !canAttach} title={!canAttach ? t("remote.capabilityUnavailable") : !runtime?.gateway.running ? t("pool.start") : t("gateway.remoteClientSwitchHint")} onClick={() => void switchRemote()}>{t("gateway.remoteClientSwitch")}</Button>
    </section>;
  }

  if (mode !== "local") return null;

  const automaticUnavailable = codexPoolOauthSelection === "auto" && !eligibleAccounts.length;
  const accountOptions = [
    { value: "auto", label: t("gateway.oauthBindingAutomatic") },
    ...eligibleAccounts.map((account) => ({ value: account.id, label: `${account.label} · ${formatAccountPlan(account.subscription.planType, t("common.unknown"))}` })),
    { value: "none", label: t("gateway.oauthBindingNone") },
  ];
  const selectedOauthAccountId = codexPoolOauthSelection !== "none" && codexPoolOauthSelection !== "auto"
    && eligibleAccounts.some((account) => account.id === codexPoolOauthSelection)
    ? codexPoolOauthSelection
    : null;
  const switchNow = () => activateCodexProfile(
    "gateway-client-switch",
    () => relayCommands.attachCodexGateway(selectedOauthAccountId, codexPoolOauthSelection === "none"),
    true,
  );

  return <section className="gateway-account-panel client-setup codex-client-setup client-oauth-binding">
    <header>
      <span className="gateway-config-icon"><UserRound aria-hidden /></span>
      <div><h2>{t("gateway.oauthBinding")}</h2><p>{t("gateway.oauthBindingHint")}</p></div>
    </header>
    <div className="oauth-binding-settings">
      <div className="relay-field oauth-binding-account-control">
        <OptionMenu className="field-option-menu" label={t("gateway.oauthBindingAccount")} value={codexPoolOauthSelection} onChange={setCodexPoolOauthSelection} options={accountOptions} />
      </div>
      <Button className="oauth-binding-switch" variant="secondary" icon={<ArrowRightLeft aria-hidden />} busy={busy === "gateway-client-switch"} disabled={!runtime?.gateway.running} title={!runtime?.gateway.running ? t("pool.start") : t("gateway.oauthBindingSwitchHint")} onClick={() => void switchNow()}>{t("gateway.oauthBindingSwitch")}</Button>
      {automaticUnavailable ? <small className="oauth-binding-selection-hint warning"><CircleAlert aria-hidden /><span>{t("gateway.oauthBindingUnavailable")}</span></small> : null}
    </div>
    {codexPoolOauthSelection !== "none" ? <SettingToggle className="oauth-binding-reserve-toggle" label={t("gateway.oauthBindingReserve")} description={t("gateway.oauthBindingReserveHint")} checked={reserveEnabled} disabled={busy === "chatgpt-quota-reserve"} onChange={(checked) => void perform("chatgpt-quota-reserve", () => relayCommands.updateChatgptQuotaReserve(checked ? 100 : 0), "feedback.saved")} /> : null}
  </section>;
}
