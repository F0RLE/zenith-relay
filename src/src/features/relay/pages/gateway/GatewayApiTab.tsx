import { useEffect, useState } from "react";
import { CheckCircle2, CirclePause, Copy, KeyRound, Link2, RefreshCw, Save } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, Button, CopyButton, EmptyState, copyText, useConfirm } from "../../components/Ui";
import { sourcePort } from "../../sourceUrl";
import { useRelayState } from "../../state/RelayStateProvider";

export function GatewayApiTab({ running, endpoint }: { running: boolean; endpoint: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const currentPort = mode === "local" ? sourcePort(endpoint) : "";
  const [port, setPort] = useState(currentPort);
  useEffect(() => setPort(currentPort), [currentPort]);

  if (!endpoint) return <section className="gateway-tab-panel gateway-empty-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />
  </section>;

  const numericPort = Number(port);
  const portValid = Number.isInteger(numericPort) && numericPort >= 1024 && numericPort <= 65535;
  const portChanged = numericPort !== Number(currentPort);
  const savingPort = busy === "gateway-port";
  const canSavePort = portValid && portChanged && !savingPort;
  const savePort = () => {
    if (canSavePort) void perform("gateway-port", () => relayCommands.updateGatewayPort(numericPort), "feedback.saved");
  };
  const canCopyApiKey = mode === "local" || (mode === "remote" && running && Boolean(runtime?.capabilities.features.includes("profile_attach")));
  const canRotateApiKey = mode === "local" || (canCopyApiKey && Boolean(runtime?.capabilities.features.includes("profile_key_rotation")));
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

  return <section className="gateway-tab-panel gateway-api-tab" role="tabpanel" aria-label={t("gateway.tabs.api")}>
    <div className="gateway-api-connection-panel">
      <header className="gateway-api-overview">
        <div className={`gateway-api-status${running ? " running" : ""}`}>
          {running ? <CheckCircle2 aria-hidden /> : <CirclePause aria-hidden />}
          <div>
            <h2>{running ? t("gateway.runtimeOnline") : t("gateway.runtimeOffline")}</h2>
            <p>{t(`gateway.runtimeHints.${mode}`)}</p>
          </div>
        </div>
        {runtime ? <dl className="gateway-api-metrics">
          <div><dt>{t("common.models")}</dt><dd>{runtime.gateway.visibleModelIds.length}</dd></div>
          <div><dt>{t("pool.members")}</dt><dd>{runtime.gateway.candidateCount}</dd></div>
        </dl> : null}
      </header>

      <div className="gateway-api-fields">
        <div className="gateway-api-field gateway-api-endpoint">
          <h3><Link2 aria-hidden />{t("gateway.endpoint")}</h3>
          <code className="gateway-api-address">{endpoint}</code>
          <div className="gateway-api-field-actions">
            <CopyButton value={endpoint} label={t("gateway.copyEndpoint")}>
              {t("gateway.copyEndpoint")}
            </CopyButton>
          </div>
        </div>
        {mode !== "zenith" ? <div className="gateway-api-field gateway-api-key">
          <h3><KeyRound aria-hidden />{t("gateway.apiKey")}</h3>
          <p>{apiKeyDisabledHint ?? t("gateway.apiKeyHint")}</p>
          <div className="gateway-api-field-actions">
            <Button icon={<Copy aria-hidden />} busy={copyingApiKey || rotatingApiKey} disabled={!canCopyApiKey} onClick={() => void copyApiKey()}>
              {t("gateway.copyApiKey")}
            </Button>
            <ActionMenu label={t("gateway.apiKeyActions")}>
              <ActionMenuItem icon={<RefreshCw aria-hidden />} title={apiKeyRotationDisabledHint} disabled={!canRotateApiKey || copyingApiKey || rotatingApiKey} onClick={() => void rotateApiKey()}>
                {t("gateway.regenerateApiKey")}
              </ActionMenuItem>
            </ActionMenu>
          </div>
        </div> : null}
      </div>

      {mode === "local" ? <form className="gateway-api-settings" onSubmit={(event) => { event.preventDefault(); savePort(); }}>
        <div className="gateway-api-port-heading">
          <label htmlFor="gateway-api-port">{t("gateway.port")}</label>
          <p id="gateway-api-port-hint">{portChanged && running ? t("gateway.portRestartHint") : t("gateway.portHint")}</p>
        </div>
        <div className="gateway-api-port-control">
          <input id="gateway-api-port" aria-describedby="gateway-api-port-hint" type="number" min="1024" max="65535" required disabled={savingPort} value={port} onChange={(event) => setPort(event.target.value)} />
          <Button type="submit" icon={<Save aria-hidden />} aria-label={running ? t("gateway.applyRestart") : t("common.save")} disabled={!canSavePort} busy={savingPort}>{t("common.save")}</Button>
        </div>
      </form> : null}
    </div>
  </section>;
}
