import { Play, RotateCw, Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, Button, PageHeader } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { useSavedChoice } from "../../state/usePendingFlag";
import { GatewayApiTab } from "./GatewayApiTab";

export function GatewayPage() {
  const { t } = useTranslation();
  const { mode, runtime, readyState, busy, perform } = useRelayState();
  const gatewayRunning = useSavedChoice(mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running));
  const running = gatewayRunning.value;
  const endpoint = mode === "zenith" ? "https://api.zenithmarket.dev/v1" : runtime?.gateway.baseUrl ?? "";
  const canManage = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("local_gateway"));

  const restart = () => perform("gateway-restart", async () => {
    if (mode === "local") {
      await relayCommands.restartGateway();
    } else {
      await relayCommands.remoteAction({ type: "stop_gateway" });
      await relayCommands.remoteAction({ type: "start_gateway" });
    }
  }, "feedback.restarted", { backgroundRefresh: true });

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
      onClick={() => {
        const shouldRunGateway = !running;
        void perform(
          "gateway-toggle",
          async () => {
            if (mode === "local") {
              if (shouldRunGateway) await relayCommands.startGateway();
              else await relayCommands.stopGateway();
            } else await relayCommands.remoteAction({ type: shouldRunGateway ? "start_gateway" : "stop_gateway" });
            gatewayRunning.confirm(shouldRunGateway);
          },
          shouldRunGateway ? "feedback.started" : "feedback.stopped",
          { backgroundRefresh: true },
        );
      }}
    >
      {running ? t("gateway.stop") : t("gateway.start")}
    </Button>
  </>;

  return <section className="relay-page relay-workspace-page gateway-page">
    <PageHeader title={t("nav.gateway")} actions={apiActions} />
    <GatewayApiTab running={running} endpoint={endpoint} />
  </section>;
}
