import { useEffect, useState } from "react";
import { ClipboardCheck, Copy, Network, Play, RefreshCw, RotateCw, Save, Square, Wrench } from "lucide-react";
import { useTranslation } from "react-i18next";
import { getSavedKeyStats } from "../../../../tauri";
import { relayCommands } from "../../api/commands";
import type { GatewayDiagnostic, SupportBundlePreview } from "../../api/types";
import { Button, Dialog, EmptyState, PageHeader, SecretField, StatusBadge, Tabs, copyText } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type View = "endpoint" | "clients" | "diagnostics";

export function GatewayPage() {
  const { t } = useTranslation(); const { mode, runtime, readyState, busy, perform } = useRelayState(); const [view, setView] = useState<View>("endpoint"); const running = mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running); const endpoint = mode === "zenith" ? "https://api.zenithmarket.dev/v1" : runtime?.gateway.baseUrl ?? "";
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("local_gateway"));
  const restart = () => perform("gateway-restart", async () => { if (mode === "local") await relayCommands.restartGateway(); else { await relayCommands.remoteAction({type:"stop_gateway"}); await relayCommands.remoteAction({type:"start_gateway"}); } }, "feedback.restarted");
  const action = mode === "zenith" ? null : <>{running ? <Button variant="secondary" busy={busy === "gateway-restart"} disabled={!canManage} title={!canManage ? t("common.unsupported") : undefined} icon={<RotateCw aria-hidden />} onClick={restart}>{t("gateway.restart")}</Button> : null}<Button variant="primary" busy={busy === "gateway-toggle"} disabled={!canManage} title={!canManage ? t("common.unsupported") : undefined} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway-toggle", () => mode === "local" ? (running ? relayCommands.stopGateway() : relayCommands.startGateway()) : relayCommands.remoteAction({type:running?"stop_gateway":"start_gateway"}), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button></>;
  return <section className="relay-page"><PageHeader title={t("nav.gateway")} subtitle={t(`gateway.subtitles.${mode}`)} actions={action} /><Tabs value={view} onChange={(id) => setView(id as View)} label={t("gateway.views")} items={[{id:"endpoint",label:t("gateway.endpoint")},{id:"clients",label:t("gateway.clientSetup")},{id:"diagnostics",label:t("gateway.diagnostics")}]}/>{view === "endpoint" ? <EndpointView running={running} endpoint={endpoint} /> : null}{view === "clients" ? <ClientSetup endpoint={endpoint} /> : null}{view === "diagnostics" ? <Diagnostics running={running} /> : null}</section>;
}

function EndpointView({ running, endpoint }: { running: boolean; endpoint: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const currentPort = endpoint ? new URL(endpoint).port || "443" : "";
  const [port, setPort] = useState(currentPort);
  const [proxyUrl, setProxyUrl] = useState("");
  useEffect(() => setPort(currentPort), [currentPort]);
  if (!endpoint) return <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />;
  const numericPort = Number(port);
  const portValid = Number.isInteger(numericPort) && numericPort >= 1024 && numericPort <= 65535;
  const proxySupported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("account_proxies"));
  const proxyConfigured = Boolean(runtime?.gateway.commonProxyConfigured);
  const proxyAvailable = Boolean(runtime?.gateway.commonProxyAvailable);
  const proxyRequired = Boolean(runtime?.gateway.accountProxyRequired);
  const saveProxy = async (value: string | null) => {
    const ok = await perform("gateway-proxy", () => mode === "local"
      ? relayCommands.setCommonProxy(value)
      : relayCommands.remoteAction({ type: "set_common_proxy" }, { proxyUrl: value }), "feedback.saved");
    if (ok) setProxyUrl("");
  };
  const saveProxyPolicy = (required: boolean) => perform("gateway-proxy-policy", () => mode === "local"
    ? relayCommands.setAccountProxyRequired(required)
    : relayCommands.remoteAction({ type: "set_account_proxy_required" }, { required }), "feedback.saved");
  return <div className="gateway-sections">
    <section><h2 className="sr-only">{t("gateway.endpoint")}</h2><div className="endpoint-line"><code>{endpoint}</code><StatusBadge status={running ? "ready" : "warning"} label={running ? t("common.online") : t("common.offline")} /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(endpoint)}>{t("common.copy")}</Button></div></section>
    <section><h2>{t("gateway.runtime")}</h2><dl className="runtime-grid"><div><dt>{t("common.status")}</dt><dd>{running ? t("common.online") : t("common.offline")}</dd></div><div><dt>{t("common.models")}</dt><dd>{runtime?.gateway.visibleModelIds.length ?? "-"}</dd></div><div><dt>{t("pool.members")}</dt><dd>{runtime?.gateway.candidateCount ?? "-"}</dd></div><div><dt>{t("common.mode")}</dt><dd>{t(`modes.${mode}`)}</dd></div></dl></section>
    <section><h2>{t("gateway.settings")}</h2><div className="settings-row"><label><span>{t("gateway.host")}</span><input value={mode === "local" ? "127.0.0.1" : new URL(endpoint).host} disabled /></label><label><span>{t("gateway.port")}</span><input type="number" min="1024" max="65535" value={port} disabled={mode !== "local"} onChange={(event) => setPort(event.target.value)} /></label><label><span>{t("gateway.bind")}</span><select disabled><option>{mode === "local" ? t("gateway.loopback") : t("gateway.serverManaged")}</option></select></label>{mode === "local" ? <Button variant="secondary" busy={busy === "gateway-port"} disabled={!portValid || port === currentPort} icon={<Save aria-hidden />} onClick={() => perform("gateway-port", () => relayCommands.updateGatewayPort(numericPort), "feedback.saved")}>{running ? t("gateway.applyRestart") : t("common.save")}</Button> : null}</div></section>
    {mode !== "zenith" ? <section className="proxy-settings"><header><div><h2>{t("proxies.commonTitle")}</h2><p>{t("proxies.commonDescription")}</p></div><StatusBadge status={!proxySupported ? "disabled" : proxyConfigured && !proxyAvailable ? "error" : "ready"} label={!proxySupported ? t("common.unsupported") : proxyConfigured ? proxyAvailable ? t("proxies.configured") : t("proxies.unavailable") : proxyRequired ? t("proxies.directBlocked") : t("proxies.notConfigured")} /></header><div className="proxy-settings-form"><Network aria-hidden /><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder="user:password@us-proxy.example:8080" /><div className="inline-actions">{proxyConfigured ? <Button variant="secondary" busy={busy === "gateway-proxy"} disabled={!proxySupported} onClick={() => saveProxy(null)}>{t("proxies.clearCommon")}</Button> : null}<Button variant="primary" busy={busy === "gateway-proxy"} disabled={!proxySupported || !proxyUrl.trim()} onClick={() => saveProxy(proxyUrl.trim())}>{t("common.save")}</Button></div></div><label className="toggle-row proxy-egress-policy"><input type="checkbox" checked={proxyRequired} disabled={!proxySupported || busy === "gateway-proxy-policy"} onChange={(event) => void saveProxyPolicy(event.target.checked)} /><span><strong>{t("proxies.requireProxy")}</strong><small>{t("proxies.requireProxyHint")}</small></span></label><p className="form-note">{t("proxies.precedence")}</p></section> : null}
  </div>;
}

function ClientSetup({ endpoint }: { endpoint: string }) {
  const { t } = useTranslation(); const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState(); const [client, setClient] = useState("codex"); const [boundOauthAccountId, setBoundOauthAccountId] = useState(""); const key = runtime?.keys[0];
  const eligibleAccounts = (runtime?.accounts ?? []).filter((account) => account.enabled && account.secretAvailable && (typeof account.authState === "string" ? account.authState : account.authState.state) === "active");
  const eligibleAccountIds = eligibleAccounts.map((account) => account.id).join("\0");
  useEffect(() => {
    const ids = eligibleAccountIds ? eligibleAccountIds.split("\0") : [];
    setBoundOauthAccountId((current) => current && !ids.includes(current) ? "" : current);
  }, [eligibleAccountIds]);
  const config = client === "codex" ? `base_url = "${endpoint}"\nwire_api = "responses"\nrequires_openai_auth = true` : client === "opencode" ? JSON.stringify({provider:{zenith_relay_local:{npm:"@ai-sdk/openai-compatible",name:"Zenith Relay Local",options:{baseURL:endpoint}}}}, null, 2) : `OPENAI_BASE_URL=${endpoint}`;
  const attach = () => key && (client === "opencode" ? perform("profile-attach", () => relayCommands.attachOpenCodeGateway(key.id), "feedback.profileAttached") : activateCodexProfile("profile-attach", () => relayCommands.attachCodexGateway(key.id, boundOauthAccountId || null)));
  const restore = () => perform("profile-restore", client === "opencode" ? relayCommands.restoreOpenCode : relayCommands.restoreCodex, "feedback.restored");
  return <div className="client-setup">
    <div className="segmented">{["codex","opencode","other"].map((value) => <button type="button" key={value} className={client === value ? "active" : ""} onClick={() => setClient(value)}>{t(`clients.${value}`)}</button>)}</div>
    {client === "codex" && mode === "local" ? <section className="client-oauth-binding">
      <header><div><h2>{t("gateway.oauthBinding")}</h2><p>{t("gateway.oauthBindingHint")}</p></div><StatusBadge status={boundOauthAccountId ? "ready" : "warning"} label={boundOauthAccountId ? t("gateway.oauthBindingSelected") : t("gateway.oauthBindingOptional")} /></header>
      <label className="relay-field"><span>{t("gateway.oauthBindingAccount")}</span><select aria-label={t("gateway.oauthBindingAccount")} value={boundOauthAccountId} onChange={(event) => setBoundOauthAccountId(event.target.value)}><option value="">{t("gateway.oauthBindingNone")}</option>{eligibleAccounts.map((account) => <option key={account.id} value={account.id}>{account.label}{account.subscription.planType ? ` - ${account.subscription.planType.toUpperCase()}` : ""}</option>)}</select></label>
      {!eligibleAccounts.length ? <p className="form-note warning-text">{t("gateway.oauthBindingUnavailable")}</p> : null}
    </section> : null}
    <section className="config-preview"><header><h2>{t("gateway.generatedConfig")}</h2><Button variant="ghost" icon={<Copy aria-hidden />} onClick={() => copyText(config)}>{t("common.copy")}</Button></header><pre>{config}</pre></section>
    {client !== "other" ? <div className="inline-actions"><Button variant="primary" busy={busy === "profile-attach"} disabled={mode !== "local" || !key} icon={<ClipboardCheck aria-hidden />} onClick={attach}>{t("profiles.attachCurrent")}</Button><Button variant="secondary" busy={busy === "profile-restore"} disabled={mode !== "local"} onClick={restore}>{t("profiles.restore")}</Button></div> : null}
    <p className="form-note">{mode !== "local" || client === "other" ? t("gateway.manualOnly") : t("profiles.backupHint")}</p>
  </div>;
}

function Diagnostics({ running }: { running: boolean }) {
  const { t } = useTranslation(); const { mode, runtime, localUsage, remoteUsage, readyUsage, perform, busy } = useRelayState(); const [result, setResult] = useState<GatewayDiagnostic | null>(null); const [supportPreview, setSupportPreview] = useState<SupportBundlePreview | null>(null);
  const supportContext = { mode, schemaVersion: runtime?.schemaVersion ?? null, gatewayRunning: Boolean(runtime?.gateway.running), sourceCount: runtime?.sources.length ?? 0, accountCount: runtime?.accounts.length ?? 0, keyCount: runtime?.keys.length ?? 0, automationCount: runtime?.automations.length ?? 0, usageCount: mode === "local" ? localUsage.length : mode === "remote" ? remoteUsage.length : readyUsage.length, warningCount: runtime?.warnings.length ?? 0 };
  const remoteDiagnostics = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("diagnostics"));
  const run = (stream: boolean) => perform(stream ? "diagnostics-stream" : "diagnostics", async () => { if (mode === "local") setResult(await relayCommands.diagnoseGateway(stream)); else if (mode === "remote") setResult(await relayCommands.diagnoseRemoteGateway(stream)); else await getSavedKeyStats(); }, "feedback.checked");
  const healthDisabled = !running || !remoteDiagnostics;
  const healthReason = !running ? t("gateway.startForDiagnostics") : !remoteDiagnostics ? t("remote.capabilityUnavailable") : undefined;
  const streamDisabled = !running || mode === "zenith" || !remoteDiagnostics;
  const streamReason = !running ? t("gateway.startForDiagnostics") : mode === "zenith" ? t("gateway.readyStreamUnsupported") : !remoteDiagnostics ? t("gateway.remoteStreamUnsupported") : undefined;
  const previewSupport = async () => {
    let preview: SupportBundlePreview | null = null;
    const ok = await perform("support-preview", async () => { preview = await relayCommands.previewSupportBundle(supportContext); });
    if (ok) setSupportPreview(preview);
  };
  const exportSupport = async () => {
    const ok = await perform("support-export", () => relayCommands.exportSupportBundle(supportContext), "feedback.exported");
    if (ok) setSupportPreview(null);
  };
  return <><div className="diagnostics-list"><section><Wrench aria-hidden /><div><strong>{t("gateway.endpointHealth")}</strong><span>{result && !result.stream ? t("gateway.diagnosticResult", { model: result.model, latency: result.latencyMs }) : running ? t("common.ready") : t("common.offline")}</span></div><Button variant="secondary" busy={busy === "diagnostics"} disabled={healthDisabled} title={healthReason} onClick={() => run(false)}>{t("common.run")}</Button></section><section><RefreshCw aria-hidden /><div><strong>{t("gateway.streamTest")}</strong><span>{result?.stream ? t("gateway.diagnosticResult", { model: result.model, latency: result.latencyMs }) : t("gateway.streamHint")}</span></div><Button variant="secondary" busy={busy === "diagnostics-stream"} disabled={streamDisabled} title={streamReason} onClick={() => run(true)}>{t("common.run")}</Button></section><section><ClipboardCheck aria-hidden /><div><strong>{t("gateway.supportBundle")}</strong><span>{t("gateway.redactedLogs")}</span></div><Button variant="secondary" busy={busy === "support-preview"} onClick={previewSupport}>{t("gateway.previewSupport")}</Button></section></div>{supportPreview ? <Dialog title={t("gateway.supportPreviewTitle")} onClose={() => setSupportPreview(null)} footer={<><Button variant="secondary" onClick={() => setSupportPreview(null)}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "support-export"} onClick={exportSupport}>{t("common.export")}</Button></>}><p>{t("gateway.supportPreviewHint")}</p><dl className="detail-list"><div><dt>{t("common.mode")}</dt><dd>{t(`modes.${supportPreview.bundle.mode}`)}</dd></div><div><dt>{t("gateway.schemaVersion")}</dt><dd>{supportPreview.bundle.schemaVersion ?? t("common.unknown")}</dd></div><div><dt>{t("gateway.sourceCount")}</dt><dd>{supportPreview.bundle.sourceCount}</dd></div><div><dt>{t("gateway.accountCount")}</dt><dd>{supportPreview.bundle.accountCount}</dd></div><div><dt>{t("gateway.keyCount")}</dt><dd>{supportPreview.bundle.keyCount}</dd></div><div><dt>{t("gateway.automationCount")}</dt><dd>{supportPreview.bundle.automationCount}</dd></div><div><dt>{t("gateway.usageCount")}</dt><dd>{supportPreview.bundle.usageCount}</dd></div><div><dt>{t("gateway.warningCount")}</dt><dd>{supportPreview.bundle.warningCount}</dd></div></dl><strong>{t("gateway.excludedData")}</strong><ul>{supportPreview.excluded.map((item) => <li key={item}>{t(`gateway.excluded.${item}`)}</li>)}</ul></Dialog> : null}</>;
}
