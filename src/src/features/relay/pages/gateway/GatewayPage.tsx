import { useEffect, useState } from "react";
import { ArrowRightLeft, CheckCircle2, CircleAlert, Network, Play, RotateCw, Save, Square, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, Button, EmptyState, OptionMenu, PageHeader, SettingToggle, formatAccountPlan, isCodexOauthAccountEligible } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { LocalClientAccess, RemoteClientAccess } from "./ClientAccess";

export function GatewayPage() {
  const { t } = useTranslation(); const { mode, runtime, readyState, busy, perform } = useRelayState(); const running = mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running); const endpoint = mode === "zenith" ? "https://api.zenithmarket.dev/v1" : runtime?.gateway.baseUrl ?? "";
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("local_gateway"));
  const restart = () => perform("gateway-restart", async () => { if (mode === "local") await relayCommands.restartGateway(); else { await relayCommands.remoteAction({type:"stop_gateway"}); await relayCommands.remoteAction({type:"start_gateway"}); } }, "feedback.restarted");
  const action = mode === "zenith" ? null : <><ActionMenu><ActionMenuItem icon={<RotateCw aria-hidden />} disabled={!running || !canManage || busy === "gateway-restart"} onClick={restart}>{t("gateway.restart")}</ActionMenuItem></ActionMenu><Button variant={running ? "secondary" : "primary"} busy={busy === "gateway-toggle"} disabled={!canManage} title={!canManage ? t("common.unsupported") : undefined} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway-toggle", () => mode === "local" ? (running ? relayCommands.stopGateway() : relayCommands.startGateway()) : relayCommands.remoteAction({type:running?"stop_gateway":"start_gateway"}), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button></>;
  return <section className="relay-page"><PageHeader title={t("nav.gateway")} subtitle={t(`gateway.subtitles.${mode}`)} actions={action} /><GatewayWorkspace running={running} endpoint={endpoint} /></section>;
}

function GatewayWorkspace({ running, endpoint }: { running: boolean; endpoint: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const currentPort = mode === "local" && endpoint ? new URL(endpoint).port : "";
  const [port, setPort] = useState(currentPort);
  useEffect(() => setPort(currentPort), [currentPort]);
  if (!endpoint) return <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />;
  const numericPort = Number(port);
  const portValid = Number.isInteger(numericPort) && numericPort >= 1024 && numericPort <= 65535;
  const savePort = () => perform("gateway-port", () => relayCommands.updateGatewayPort(numericPort), "feedback.saved");
  return <div className="gateway-workspace">
    <section className="gateway-runtime-panel"><div className={`gateway-runtime-state${running ? " running" : ""}`}><span className="gateway-runtime-icon">{running ? <CheckCircle2 aria-hidden /> : <CircleAlert aria-hidden />}</span><div><h2>{running ? t("gateway.runtimeOnline") : t("gateway.runtimeOffline")}</h2><p>{t(`gateway.runtimeHints.${mode}`)}</p></div></div>{runtime ? <dl className="gateway-runtime-metrics"><div><dt>{t("common.models")}</dt><dd>{runtime.gateway.visibleModelIds.length}</dd></div><div><dt>{t("pool.members")}</dt><dd>{runtime.gateway.candidateCount}</dd></div></dl> : null}</section>
    {mode !== "zenith" ? <div className="gateway-settings-panel">{mode === "local" ? <section className="gateway-setting-row gateway-port-section"><header><span className="gateway-config-icon"><Network aria-hidden /></span><div><h2>{t("gateway.settings")}</h2><p>{t("gateway.portHint")}</p></div></header><form className="gateway-port-control" onSubmit={(event) => { event.preventDefault(); void savePort(); }}><label><span>{t("gateway.port")}</span><input type="number" min="1024" max="65535" value={port} onChange={(event) => setPort(event.target.value)} /></label><Button type="submit" variant="secondary" busy={busy === "gateway-port"} disabled={!portValid || port === currentPort} icon={<Save aria-hidden />}>{running ? t("gateway.applyRestart") : t("common.save")}</Button></form></section> : null}<ClientSetup />{mode === "local" ? <LocalClientAccess /> : null}{mode === "remote" ? <RemoteClientAccess /> : null}</div> : null}
  </div>;
}

function ClientSetup() {
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
      <header><span className="gateway-config-icon"><UserRound aria-hidden /></span><div><h2>{t("gateway.clientSetup")}</h2><p>{t("gateway.remoteClientHint")}</p></div></header>
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
    <header><span className="gateway-config-icon"><UserRound aria-hidden /></span><div><h2>{t("gateway.oauthBinding")}</h2><p>{t("gateway.oauthBindingHint")}</p></div></header>
    <div className="oauth-binding-settings">
      <div className="relay-field oauth-binding-account-control"><span>{t("gateway.oauthBindingAccount")}</span><OptionMenu className="field-option-menu" label={t("gateway.oauthBindingAccount")} value={codexPoolOauthSelection} onChange={setCodexPoolOauthSelection} options={accountOptions} /></div>
      <Button className="oauth-binding-switch" variant="secondary" icon={<ArrowRightLeft aria-hidden />} busy={busy === "gateway-client-switch"} disabled={!runtime?.gateway.running} title={!runtime?.gateway.running ? t("pool.start") : t("gateway.oauthBindingSwitchHint")} onClick={() => void switchNow()}>{t("gateway.oauthBindingSwitch")}</Button>
      {automaticUnavailable ? <small className="oauth-binding-selection-hint warning"><CircleAlert aria-hidden /><span>{t("gateway.oauthBindingUnavailable")}</span></small> : null}
      {codexPoolOauthSelection !== "none" ? <SettingToggle className="oauth-binding-reserve-toggle" label={t("gateway.oauthBindingReserve")} description={t("gateway.oauthBindingReserveHint")} checked={reserveEnabled} disabled={busy === "chatgpt-quota-reserve"} onChange={(checked) => void perform("chatgpt-quota-reserve", () => relayCommands.updateChatgptQuotaReserve(checked ? 100 : 0), "feedback.saved")} /> : null}
    </div>
  </section>;
}
