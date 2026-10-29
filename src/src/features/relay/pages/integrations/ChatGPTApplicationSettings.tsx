import { ArrowRightLeft, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { formatAccountPlan } from "../../accountPlans";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { Button, OptionMenu, SettingToggle } from "../../components/Ui";
import { CodexBackgroundTasksControl } from "../../components/CodexBackgroundTasksControl";
import { CodexMemoryModelControl } from "../../components/CodexMemoryModelControl";
import { CodexWebsocketsControl } from "../../components/CodexWebsocketsControl";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";
import { ApplicationLaunchPath } from "./ApplicationLaunchPath";

export function ChatGPTApplicationSettings() {
  const { t } = useTranslation();
  const { mode } = useRelayState();
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.chatgpt")}>
    <ApplicationLaunchPath application="chatgpt" />
    <div className="integration-chatgpt-settings">
      <ChatGPTSetup />
      {mode === "local" ? <div className="gateway-settings-panel gateway-application-panel">
        <CodexBackgroundTasksControl className="gateway-setting-row" />
        <CodexMemoryModelControl stage="extraction" />
        <CodexMemoryModelControl stage="consolidation" />
        <CodexWebsocketsControl className="gateway-setting-row" />
      </div> : null}
    </div>
  </section>;
}

function ChatGPTSetup() {
  const { t } = useTranslation();
  const { runtime, busy, perform, activateCodexProfile, codexPoolOauthSelection, setCodexPoolOauthSelection } = useRelayState();
  const eligibleAccounts = (runtime?.accounts ?? [])
    .filter(isCodexOauthAccountEligible)
    .sort((left, right) => left.label.localeCompare(right.label) || left.id.localeCompare(right.id));
  const reserveEnabled = (runtime?.gateway.chatgptInterfaceQuotaReserveBasisPoints ?? 100) > 0;
  const reserve = usePendingFlag(reserveEnabled);

  const selectedAccount = eligibleAccounts.find((account) => account.id === codexPoolOauthSelection);
  const selection = !eligibleAccounts.length || codexPoolOauthSelection === "none"
    ? "none"
    : selectedAccount?.id ?? "none";
  const accountOptions = [
    ...eligibleAccounts.map((account) => ({ value: account.id, label: `${account.label} · ${formatAccountPlan(account.subscription.planType, t("common.unknown"))}` })),
    { value: "none", label: t("gateway.oauthBindingNone") },
  ];
  const switchNow = () => activateCodexProfile(
    "gateway-client-switch",
    () => relayCommands.attachCodexGateway(selectedAccount?.id ?? null, selection === "none"),
    true,
  );

  return <section className="gateway-account-panel client-setup codex-client-setup client-oauth-binding">
    <header>
      <span className="gateway-config-icon"><UserRound aria-hidden /></span>
      <div><h2>{t("gateway.oauthBinding")}</h2><p>{t("gateway.oauthBindingDescription")}</p></div>
    </header>
    <div className="oauth-binding-settings">
      <div className="relay-field oauth-binding-account-control">
        <OptionMenu className="field-option-menu" label={t("gateway.oauthBindingAccount")} value={selection} onChange={setCodexPoolOauthSelection} options={accountOptions} disabled={!eligibleAccounts.length} />
      </div>
      <Button
        className="oauth-binding-switch"
        variant="secondary"
        icon={<ArrowRightLeft aria-hidden />}
        busy={busy === "gateway-client-switch"}
        disabled={!runtime?.gateway.running}
        title={!runtime?.gateway.running ? t("pool.start") : t("gateway.oauthBindingSwitchHint")}
        onClick={() => void switchNow()}
      >
        {t("gateway.oauthBindingSwitch")}
      </Button>
    </div>
    {selection !== "none" ? (
      <SettingToggle
        className="oauth-binding-reserve-toggle"
        label={t("gateway.oauthBindingReserve")}
        description={t("gateway.oauthBindingReserveHint")}
        checked={reserve.checked}
        onChange={(checked) => reserve.select(checked, () => perform(
          "chatgpt-quota-reserve",
          () => relayCommands.updateChatgptQuotaReserve(checked ? 100 : 0),
          "feedback.saved",
          { backgroundRefresh: true, uiLock: false },
        ))}
      />
    ) : null}
  </section>;
}
