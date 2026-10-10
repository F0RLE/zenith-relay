import { ArrowRightLeft, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { isCodexOauthAccountEligible } from "../../accountStatus";
import { Button, EmptyState, OptionMenu, SettingToggle, formatAccountPlan } from "../../components/Ui";
import { CodexBackgroundTasksControl } from "../../components/CodexBackgroundTasksControl";
import { CodexWebsocketsControl } from "../../components/CodexWebsocketsControl";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

export function ChatGPTApplicationSettings() {
  const { t } = useTranslation();
  const { mode, runtime } = useRelayState();
  if (mode === "zenith") return <EmptyState title={t("gateway.emptyTitle")} description={t("gateway.emptyDescription")} />;
  const showSettings = mode === "local" || runtime?.capabilities.features.some((feature) =>
    feature === "codex_background_tasks" || feature === "codex_websockets",
  );
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.chatgpt")}>
    <div className="integration-chatgpt-settings">
      <ChatGPTSetup />
      {showSettings ? <div className="gateway-settings-panel gateway-application-panel">
        <CodexBackgroundTasksControl className="gateway-setting-row" />
        <CodexWebsocketsControl className="gateway-setting-row" />
      </div> : null}
    </div>
  </section>;
}

function ChatGPTSetup() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, activateCodexProfile, codexPoolOauthSelection, setCodexPoolOauthSelection } = useRelayState();
  const eligibleAccounts = (runtime?.accounts ?? [])
    .filter(isCodexOauthAccountEligible)
    .sort((left, right) => left.label.localeCompare(right.label) || left.id.localeCompare(right.id));
  const reserveEnabled = (runtime?.gateway.chatgptInterfaceQuotaReserveBasisPoints ?? 100) > 0;
  const reserve = usePendingFlag(reserveEnabled);

  if (mode === "remote") {
    const canAttach = Boolean(runtime?.capabilities.features.includes("profile_attach"));
    const switchRemote = () => activateCodexProfile("gateway-client-switch", relayCommands.attachCodexRemoteGateway, true);
    return <section className="gateway-account-panel client-setup codex-client-setup client-oauth-binding remote-client-setup">
      <header>
        <span className="gateway-config-icon"><UserRound aria-hidden /></span>
        <div><h2>{t("gateway.clientSetup")}</h2><p>{t("gateway.remoteClientHint")}</p></div>
      </header>
      <Button
        variant="secondary"
        icon={<ArrowRightLeft aria-hidden />}
        busy={busy === "gateway-client-switch"}
        disabled={!runtime?.gateway.running || !canAttach}
        title={!canAttach
          ? t("remote.capabilityUnavailable")
          : !runtime?.gateway.running
            ? t("pool.start")
            : t("gateway.remoteClientSwitchHint")}
        onClick={() => void switchRemote()}
      >
        {t("gateway.remoteClientSwitch")}
      </Button>
    </section>;
  }

  if (mode !== "local") return null;

  const selectedAccount = eligibleAccounts.find((account) => account.id === codexPoolOauthSelection);
  const selection = !eligibleAccounts.length || codexPoolOauthSelection === "none"
    ? "none"
    : selectedAccount?.id ?? "auto";
  const accountOptions = [
    ...(eligibleAccounts.length ? [{ value: "auto", label: t("gateway.oauthBindingAutomatic") }] : []),
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
      <div><h2>{t("gateway.oauthBinding")}</h2></div>
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
