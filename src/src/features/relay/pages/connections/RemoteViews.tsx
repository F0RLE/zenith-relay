import { useState } from "react";
import { Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, Dialog, EmptyState, SecretField, SettingToggle, StatusBadge, copyText, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
export function RemoteView({ onConnect, onDeploy }: { onConnect: () => void; onDeploy: () => void }) {
  const { t } = useTranslation();
  const { runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  const disconnect = async () => {
    let linkedAccounts = 0;
    const counted = await perform("remote-disconnect-check", async () => { linkedAccounts = await relayCommands.remoteLinkedAccountCount(); });
    if (!counted) return;
    const message = linkedAccounts
      ? t("remote.disconnectLinkedConfirm", { count: linkedAccounts })
      : t("remote.disconnectConfirm");
    if (await confirm(message, { danger: true })) {
      await perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected");
    }
  };
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.runtimeTarget.connected ? "ready" : "error"} label={runtime.runtimeTarget.connected ? t("common.connected") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="danger" busy={busy === "remote-disconnect-check" || busy === "remote-disconnect"} onClick={() => void disconnect()}>{t("remote.disconnect")}</Button></div></section>;
}

export function RemoteDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [allowInsecure, setAllowInsecure] = useState(false);
  const [confirmIdentityChange, setConfirmIdentityChange] = useState(false);
  const normalizedBaseUrl = baseUrl.trim();
  const insecure = normalizedBaseUrl.toLowerCase().startsWith("http://");
  const connect = async () => { const ok = await perform("remote-connect", () => relayCommands.connectRemote({ baseUrl: normalizedBaseUrl, managementToken: token, allowInsecureHttp: insecure && allowInsecure, confirmIdentityChange }), "feedback.connected"); if (ok) onClose(); };
  return <Dialog title={t("remote.connectExisting")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "remote-connect"} disabled={!normalizedBaseUrl || !token || (insecure && !allowInsecure)} onClick={connect}>{t("remote.testAndConnect")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={token} onChange={setToken} /><div className="remote-connect-options">{insecure ? <SettingToggle tone="warning" label={t("remote.allowInsecure")} description={t("remote.allowInsecureHint")} checked={allowInsecure} onChange={setAllowInsecure} /> : null}<SettingToggle label={t("remote.confirmIdentityChange")} description={t("remote.identityHint")} checked={confirmIdentityChange} onChange={setConfirmIdentityChange} /></div></div></Dialog>;
}

export function DeployDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [url, setUrl] = useState("");
  const [plan, setPlan] = useState<{ directory: string; managementToken: string; vaultKey: string; composeCommand: string } | null>(null);
  const generate = async () => { const result: { current: typeof plan } = { current: null }; const ok = await perform("remote-deploy", async () => { result.current = await relayCommands.prepareRemoteDeployment(url); }, "feedback.deploymentPrepared"); if (ok) setPlan(result.current); };
  return <Dialog title={t("remote.deployNew")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>{!plan ? <Button variant="primary" busy={busy === "remote-deploy"} disabled={!url} onClick={generate}>{t("remote.generate")}</Button> : null}</>}>{plan ? <div className="deployment-result"><StatusBadge status="ready" label={t("common.ready")} /><label><span>{t("remote.bundlePath")}</span><code>{plan.directory}</code></label><div className="relay-field"><span>{t("remote.token")}</span><div className="endpoint-line"><input aria-label={t("remote.token")} type="password" value={plan.managementToken} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.managementToken)}>{t("common.copy")}</Button></div></div><div className="relay-field"><span>{t("remote.vaultKey")}</span><div className="endpoint-line"><input aria-label={t("remote.vaultKey")} type="password" value={plan.vaultKey} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.vaultKey)}>{t("common.copy")}</Button></div></div><label><span>{t("remote.command")}</span><code>{plan.composeCommand}</code></label><p>{t("remote.secretOnce")}</p><p>{t("remote.deployHint")}</p></div> : <label className="relay-field"><span>{t("remote.publicUrl")}</span><input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://relay.example.com" /></label>}</Dialog>;
}
