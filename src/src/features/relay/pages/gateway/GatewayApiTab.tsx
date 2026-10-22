import { useEffect, useState, type ReactNode } from "react";
import { CheckCircle2, CirclePause, Copy, KeyRound, Laptop, Link2, RefreshCw, Save } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, CopyButton, EmptyState, IconButton, copyText, useConfirm } from "../../components/Ui";
import { sourcePort } from "../../sourceUrl";
import { useRelayState } from "../../state/RelayStateProvider";
import { RouteRecoveryControl } from "./RouteRecoveryControl";
import { NetworkAccessControl } from "./NetworkAccessControl";

export function GatewayApiTab({ running, endpoint, actions }: { running: boolean; endpoint: string; actions: ReactNode }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const connectionTitle = t(mode === "local" ? "gateway.localConnection" : "gateway.connection");
  const currentPort = mode === "local" ? sourcePort(endpoint) : "";
  const [port, setPort] = useState(currentPort);
  useEffect(() => setPort(currentPort), [currentPort]);

  if (!endpoint) return <section className="gateway-tab-panel gateway-empty-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <div className="integration-section-heading">
      <h2>{t("integrations.apiTitle")}</h2>
      <div className="integration-section-actions">{actions}</div>
    </div>
    <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />
  </section>;

  const numericPort = Number(port);
  const portValid = Number.isInteger(numericPort) && numericPort >= 1024 && numericPort <= 65535;
  const portChanged = numericPort !== Number(currentPort);
  const savingPort = busy === "gateway-port";
  const canSavePort = portValid && portChanged && !savingPort;
  const savePort = () => {
    if (canSavePort) void perform("gateway-port", () => relayCommands.updateGatewayPort(numericPort), "feedback.saved", { backgroundRefresh: true });
  };
  const canCopyApiKey = mode === "local";
  const canRotateApiKey = mode === "local";
  const copyingApiKey = busy === "gateway-api-key";
  const rotatingApiKey = busy === "gateway-api-key-rotate";
  const copyApiKey = () => perform("gateway-api-key", async () => {
    await copyText(await relayCommands.revealLocalGatewayApiKey());
  }, "feedback.copied", { backgroundRefresh: true });
  const rotateApiKey = async () => {
    if (!await confirm(t("gateway.regenerateApiKeyConfirm"), {
      title: t("gateway.regenerateApiKey"),
      confirmLabel: t("gateway.regenerateApiKey"),
      danger: true,
    })) return;
    await perform("gateway-api-key-rotate", async () => {
      await copyText(await relayCommands.rotateLocalGatewayApiKey());
    }, "feedback.copied", { backgroundRefresh: true });
  };

  return <section className="gateway-tab-panel gateway-api-tab" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <div className="gateway-api-connection-panel">
      <header className="gateway-api-overview">
        <div className={`gateway-api-status${running ? " running" : ""}`}>
          {running ? <CheckCircle2 aria-hidden /> : <CirclePause aria-hidden />}
          <div>
            <h2>{running ? t("gateway.runtimeOnline") : t("gateway.runtimeOffline")}</h2>
            {runtime ? <dl className="gateway-api-metrics">
              <div><dt>{t("common.models")}</dt><dd>{runtime.gateway.visibleModelIds.length}</dd></div>
              <div><dt>{t("pool.members")}</dt><dd>{runtime.gateway.candidateCount}</dd></div>
            </dl> : null}
          </div>
        </div>
        <div className="gateway-api-controls">{actions}</div>
      </header>

      <section className="gateway-api-section gateway-api-local" aria-label={connectionTitle}>
        <header className="gateway-api-section-heading">
          {mode === "local" ? <Laptop aria-hidden /> : <Link2 aria-hidden />}
          <h3>{connectionTitle}</h3>
        </header>
        <div className="gateway-api-fields">
          <div className="gateway-api-field gateway-api-endpoint">
            <h4><Link2 aria-hidden />{t("gateway.endpoint")}</h4>
            <div className="gateway-api-value">
              <code className="gateway-api-address">{endpoint}</code>
              <CopyButton value={endpoint} label={t("gateway.copyEndpoint")} />
            </div>
          </div>
          <div className="gateway-api-field gateway-api-key">
            <h4><KeyRound aria-hidden />{t("gateway.apiKey")}</h4>
            <div className="gateway-api-value">
              <span className="gateway-api-secret" aria-label={t("gateway.keyHidden")}>•••• •••• ••••</span>
              <div className="gateway-api-field-actions">
                <IconButton label={t("gateway.copyApiKey")} icon={<Copy aria-hidden />} busy={copyingApiKey || rotatingApiKey} disabled={!canCopyApiKey} onClick={() => void copyApiKey()} />
                <ActionMenu label={t("gateway.apiKeyActions")}>
                  <ActionMenuItem icon={<RefreshCw aria-hidden />} disabled={!canRotateApiKey || copyingApiKey || rotatingApiKey} onClick={() => void rotateApiKey()}>
                    {t("gateway.regenerateApiKey")}
                  </ActionMenuItem>
                </ActionMenu>
              </div>
            </div>
            <p>{t("gateway.apiKeyHint")}</p>
          </div>
        </div>

        {mode === "local" ? <form className="gateway-api-settings" onSubmit={(event) => { event.preventDefault(); savePort(); }}>
          <div className="gateway-api-port-heading">
            <label htmlFor="gateway-api-port">{t("gateway.port")}</label>
            <p id="gateway-api-port-hint">{portChanged && running ? t("gateway.portRestartHint") : t("gateway.portHint")}</p>
          </div>
          <div className="gateway-api-port-control">
            <input id="gateway-api-port" aria-describedby="gateway-api-port-hint" type="number" min="1024" max="65535" required disabled={savingPort} value={port} onChange={(event) => setPort(event.target.value)} />
            <IconButton type="submit" icon={<Save aria-hidden />} label={running ? t("gateway.applyRestart") : t("common.save")} disabled={!canSavePort} busy={savingPort} />
          </div>
        </form> : null}
      </section>
      {mode === "local" ? <NetworkAccessControl port={currentPort} running={running} /> : null}
      <RouteRecoveryControl />
    </div>
  </section>;
}
