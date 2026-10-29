import { Play, RotateCw, Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { ActionMenu, ActionMenuItem, Button } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { useSavedChoice } from "../../state/usePendingFlag";
import { GatewayApiTab } from "./GatewayApiTab";

export function GatewaySettings() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const gatewayRunning = useSavedChoice(Boolean(runtime?.gateway.running));
  const running = gatewayRunning.value;
  const endpoint = runtime?.gateway.baseUrl ?? "";
  const canManage = mode === "local";

  const restart = () => perform("gateway-restart", relayCommands.restartGateway, "feedback.restarted", { backgroundRefresh: true });

  const apiActions = <>
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
            if (shouldRunGateway) await relayCommands.startGateway();
            else await relayCommands.stopGateway();
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

  return <div className="gateway-page">
    <GatewayApiTab running={running} endpoint={endpoint} actions={apiActions} />
  </div>;
}
