import { useEffect, useState } from "react";
import { ArrowRightLeft, CheckCircle2, CircleAlert, Copy, KeyRound, Loader2, Network, Play, Plug, RefreshCw, RotateCw, Save, Square, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { ActionMenu, ActionMenuItem, Button, CopyButton, EmptyState, IconButton, OptionMenu, PageHeader, SettingToggle, Tabs, copyText, formatAccountPlan, useConfirm } from "../../components/Ui";
import { CodexBackgroundTasksControl } from "../../components/CodexBackgroundTasksControl";
import { CodexWebsocketsControl } from "../../components/CodexWebsocketsControl";
import { useRelayState } from "../../state/RelayStateProvider";

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

  return <section className="relay-page gateway-page">
    <PageHeader
      title={t("nav.gateway")}
      subtitle={t(`gateway.tabSubtitles.${activeTab}.${mode}`)}
      actions={activeTab === "api" || activeTab === "chatgpt" ? (activeTab === "api" ? apiActions : chatGptActions) : null}
    />
    <Tabs value={activeTab} onChange={(value) => setActiveTab(value as GatewayTab)} label={t("gateway.tabs.label")} items={tabs} />
    {activeTab === "api"
      ? <GatewayApiTab running={running} endpoint={endpoint} />
      : activeTab === "chatgpt" ? <GatewayChatGPTTab /> : <GatewayOpenCodeTab />}
  </section>;
}

function GatewayApiTab({ running, endpoint }: { running: boolean; endpoint: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const currentPort = mode === "local" && endpoint ? new URL(endpoint).port : "";
  const [port, setPort] = useState(currentPort);
  useEffect(() => setPort(currentPort), [currentPort]);

  if (!endpoint) return <section className="gateway-tab-panel gateway-empty-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />
  </section>;

  const numericPort = Number(port);
  const portValid = Number.isInteger(numericPort) && numericPort >= 1024 && numericPort <= 65535;
  const savePort = () => perform("gateway-port", () => relayCommands.updateGatewayPort(numericPort), "feedback.saved");
  const canCopyApiKey = mode === "local" || (mode === "remote" && running && Boolean(runtime?.capabilities.features.includes("profile_attach")));
  const canRotateApiKey = mode === "local" || (mode === "remote" && running && Boolean(runtime?.capabilities.features.includes("profile_attach")) && Boolean(runtime?.capabilities.features.includes("profile_key_rotation")));
  const copyingApiKey = busy === "gateway-api-key";
  const rotatingApiKey = busy === "gateway-api-key-rotate";
  const apiKeyDisabledHint = mode === "remote" && !canCopyApiKey
    ? (running ? t("gateway.apiKeyUnavailable") : t("gateway.start"))
    : undefined;
  const apiKeyRotationDisabledHint = mode === "remote" && !canRotateApiKey
    ? (running ? t("gateway.apiKeyRotationUnavailable") : t("gateway.start"))
    : undefined;
  const copyApiKey = () => perform("gateway-api-key", async () => {
    await copyText(mode === "local"
      ? await relayCommands.revealLocalGatewayApiKey()
      : await relayCommands.revealRemoteGatewayApiKey());
  }, "feedback.copied");
  const rotateApiKey = async () => {
    if (!await confirm(t("gateway.regenerateApiKeyConfirm"), {
      title: t("gateway.regenerateApiKey"),
      confirmLabel: t("gateway.regenerateApiKey"),
      danger: true,
    })) return;
    await perform("gateway-api-key-rotate", async () => {
      await copyText(mode === "local"
        ? await relayCommands.rotateLocalGatewayApiKey()
        : await relayCommands.rotateRemoteGatewayApiKey());
    }, "feedback.copied");
  };

  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <div className="gateway-workspace">
      <GatewayRuntimePanel running={running} />
      <section className="gateway-api-connection-panel">
        <header>
          <span className="gateway-config-icon"><Network aria-hidden /></span>
          <div>
            <h2>{t("gateway.endpoint")}</h2>
          </div>
        </header>
        <div className="gateway-api-connection-controls">
          <div className="gateway-endpoint-value">
            <code title={endpoint}>{endpoint}</code>
            <CopyButton value={endpoint} label={t("gateway.copyEndpoint")} />
            {mode === "local" ? <form className="gateway-api-port-control" onSubmit={(event) => { event.preventDefault(); void savePort(); }}>
              <label>
                <span>{t("gateway.port")}</span>
                <input type="number" min="1024" max="65535" value={port} onChange={(event) => setPort(event.target.value)} />
              </label>
              <IconButton className="gateway-api-port-save" type="submit" label={running ? t("gateway.applyRestart") : t("common.save")} disabled={busy === "gateway-port" || !portValid || port === currentPort} icon={busy === "gateway-port" ? <Loader2 className="spin" aria-hidden /> : <Save aria-hidden />} />
            </form> : null}
          </div>
          {mode !== "zenith" ? <div className="gateway-api-key-control">
            <span className="gateway-api-key-label"><KeyRound aria-hidden /><span>{t("gateway.apiKey")}</span></span>
            <div className="gateway-api-key-actions">
              <IconButton className="gateway-api-key-action" label={t("gateway.copyApiKey")} title={apiKeyDisabledHint} disabled={!canCopyApiKey || rotatingApiKey || copyingApiKey} icon={copyingApiKey ? <Loader2 className="spin" aria-hidden /> : <Copy aria-hidden />} onClick={() => void copyApiKey()} />
              <IconButton className="gateway-api-key-rotate" label={t("gateway.regenerateApiKey")} title={apiKeyRotationDisabledHint} disabled={!canRotateApiKey || copyingApiKey || rotatingApiKey} icon={rotatingApiKey ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} onClick={() => void rotateApiKey()} />
            </div>
          </div> : null}
        </div>
      </section>
    </div>
  </section>;
}

function GatewayRuntimePanel({ running }: { running: boolean }) {
  const { t } = useTranslation();
  const { mode, runtime } = useRelayState();
  return <section className="gateway-runtime-panel">
    <div className={`gateway-runtime-state${running ? " running" : ""}`}>
      <span className="gateway-runtime-icon">{running ? <CheckCircle2 aria-hidden /> : <CircleAlert aria-hidden />}</span>
      <div>
        <h2>{running ? t("gateway.runtimeOnline") : t("gateway.runtimeOffline")}</h2>
        <p>{t(`gateway.runtimeHints.${mode}`)}</p>
      </div>
    </div>
    {runtime ? <dl className="gateway-runtime-metrics">
      <div><dt>{t("common.models")}</dt><dd>{runtime.gateway.visibleModelIds.length}</dd></div>
      <div><dt>{t("pool.members")}</dt><dd>{runtime.gateway.candidateCount}</dd></div>
    </dl> : null}
  </section>;
}

function GatewayChatGPTTab() {
  const { t } = useTranslation();
  const { mode } = useRelayState();
  if (mode === "zenith") return <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />;
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.chatgpt")}>
    <div className="gateway-workspace">
      <div className="gateway-settings-panel gateway-application-panel">
        <ChatGPTSetup />
        <CodexBackgroundTasksControl className="gateway-setting-row" />
        <CodexWebsocketsControl className="gateway-setting-row" />
      </div>
    </div>
  </section>;
}

function GatewayOpenCodeTab() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const [status, setStatus] = useState<import("../../api/types").OpenCodeConfigStatus | null>(null);
  const refreshStatus = () => {
    if (mode !== "local") return;
    void relayCommands.getOpenCodeConfigStatus().then(setStatus).catch(() => setStatus(null));
  };
  useEffect(refreshStatus, [mode]);
  if (mode !== "local") return <section className="gateway-tab-panel gateway-empty-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <EmptyState title={t("gateway.openCodeEmptyTitle")} description={t("gateway.openCodeEmptyDescription")} />
  </section>;
  const connect = () => perform("opencode-connect", relayCommands.connectOpenCode, "feedback.saved").then(refreshStatus);
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <div className="gateway-workspace">
      <div className="gateway-settings-panel gateway-application-panel">
        <section className="gateway-setting-row client-setup">
          <header>
            <span className="gateway-config-icon"><Plug aria-hidden /></span>
            <div><h2>{t("gateway.openCodeProviderTitle")}</h2><p>{t("gateway.openCodeProviderHint")}</p></div>
          </header>
          <div className="opencode-provider-status">
            <span className={`relay-status ${status?.configured ? "ready" : "info"}`}>
              {status?.configured ? <CheckCircle2 aria-hidden /> : <CircleAlert aria-hidden />}
              {status?.configured ? t("gateway.openCodeConfigured", { count: status.modelCount }) : t("gateway.openCodeNotConfigured")}
            </span>
            <div className="inline-actions">
              <Button variant="primary" icon={<Plug aria-hidden />} busy={busy === "opencode-connect"} disabled={!runtime?.gateway.running} title={!runtime?.gateway.running ? t("pool.start") : undefined} onClick={() => void connect()}>{t("gateway.openCodeConnect")}</Button>
            </div>
          </div>
        </section>
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
    return <section className="gateway-setting-row client-setup codex-client-setup client-oauth-binding remote-client-setup">
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

  return <section className="gateway-setting-row client-setup codex-client-setup client-oauth-binding">
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
      {codexPoolOauthSelection !== "none" ? <SettingToggle className="oauth-binding-reserve-toggle" label={t("gateway.oauthBindingReserve")} description={t("gateway.oauthBindingReserveHint")} checked={reserveEnabled} disabled={busy === "chatgpt-quota-reserve"} onChange={(checked) => void perform("chatgpt-quota-reserve", () => relayCommands.updateChatgptQuotaReserve(checked ? 100 : 0), "feedback.saved")} /> : null}
    </div>
  </section>;
}
