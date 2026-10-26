import { useRef, useState } from "react";
import type { HTMLAttributes, ReactNode } from "react";
import {
  Check,
  Download,
  ListMinus,
  ListPlus,
  Loader2,
  Network,
  Play,
  Power,
  RefreshCw,
  StickyNote,
  ShieldAlert,
  Square,
  Trash2,
  UserRound,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../../api/types";
import { accountQuotaRefreshState, accountSurfaceTone, canLaunchCodexAccount, currentAccountErrorCode, transientCandidateTone } from "../../accountStatus";
import { refreshOneAccountQuota } from "../../accountQuotaRefresh";
import { accountMetadataHint, accountMetadataIsStale, accountMetadataSources, accountSubscriptionSource } from "../../accountRoutes";
import {
  AccountBadges,
  ActionMenu,
  ActionMenuItem,
  Dialog,
  IconButton,
  StatusIcon,
  accountErrorLabel,
  useConfirm,
} from "../../components/Ui";
import { AccountValueStrip } from "../../components/AccountValueStrip";
import { AccountProviderQuotaStrip } from "../../components/AccountProviderQuotaStrip";
import { AccountQuotaPanel } from "../../components/AccountQuotaPanel";
import { AccountSubscriptionLine } from "../../components/AccountSubscriptionLine";
import { ResetCreditsControl } from "../../components/ResetCreditsControl";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { upcomingModelRetries } from "../../routingOrder";
import { canAccountJoinPool, updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";
import { accountParticipates } from "./accountTableModel";
import { AccountLoginNotes } from "../../components/AccountLoginNotes";
import { AccountTokenExpiry } from "../../components/AccountTokenExpiry";
import { AccountRouteSelector } from "../../components/AccountRouteSelector";
import { useConnectionAccountRemoval } from "./accountRemoval";

type AccountCardProps = {
  account: AccountSummary;
  nowMs: number;
  selected: boolean;
  canRefreshQuota: boolean;
  runtimeState: CandidateRuntimeSnapshot | undefined;
  onToggleSelected: (accountId: string) => void;
  onShowError: (account: AccountSummary) => void;
  onProxy: (account: AccountSummary) => void;
  onExport: (accountIds: string[]) => void;
  onReauthenticate: (account: AccountSummary, automatic?: boolean) => void;
};

export function AccountCard({
  account,
  nowMs,
  selected,
  canRefreshQuota,
  runtimeState,
  onToggleSelected,
  onShowError,
  onProxy,
  onExport,
  onReauthenticate,
}: AccountCardProps) {
  const { t, i18n } = useTranslation();
  const [notesOpen, setNotesOpen] = useState(false);
  const confirm = useConfirm();
  const membershipPending = useRef(false);
  const { mode, runtime, perform, activateCodexProfile, refresh, busy, accountIdentitiesVisible, accountValueVisible } = useRelayState();
  const metadata = accountMetadataSources(account, runtime?.accounts ?? []);
  const subscription = accountSubscriptionSource(account, runtime?.accounts ?? []);
  const { remove, openDeleteMenu, permanentDeleteMenu } = useConnectionAccountRemoval();
  const participation = usePendingFlag(accountParticipates(account));
  const enabledFlag = usePendingFlag(account.enabled);
  const participates = participation.checked;
  const accountEnabled = enabledFlag.checked;
  const recoveryRequired = Boolean(account.recoveryRequired);
  const showNotes = mode === "local" && !recoveryRequired && account.secretAvailable;
  const errorCode = recoveryRequired ? null : currentAccountErrorCode(account);
  const operationalStatus = account.operationalStatus;
  const operationalLabel = recoveryRequired ? t("accounts.recoveryRequiredHint") : t(`connections.status.${operationalStatus}`);
  const modelRetries = upcomingModelRetries(runtimeState, nowMs);
  const firstModelRetry = modelRetries[0];
  const modelRetryHint = firstModelRetry
    ? t("pool.modelRetryAt", {
      models: modelRetries.map((retry) => retry.model).join(", "),
      time: formatDetailedRemainingTime(firstModelRetry.retryAtMs, nowMs, t),
    })
    : null;
  const runtimeTone = operationalStatus === "rotation"
    ? transientCandidateTone(runtimeState, nowMs, false) ?? (modelRetries.length ? "warning" : null)
    : null;
  const runtimeHint = runtimeState?.halfOpen
    ? t("pool.recoveryProbe")
    : modelRetryHint;
  const proxyLabel = account.proxyAvailable === false && account.proxyMode === "direct" ? t("proxies.modes.blocked") : t(`proxies.modes.${account.proxyMode ?? "direct"}`);
  const poolActionLabel = participates ? t("accounts.excludeFromPool") : t(account.oauthClientKind === "codex" && !runtime?.gateway.codexTextModelsEnabled ? "accounts.includeImagesInPool" : "accounts.includeInPool");
  const quotaStatus = accountQuotaRefreshState(account);
  const clientAuthWarning = account.clientAuthStatus === "login_required";
  const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
  const canLaunch = canLaunchCodexAccount(account) && !recoveryRequired;
  const surfaceTone = accountSurfaceTone(account, recoveryRequired, runtimeState);
  const indicatorTone = surfaceTone === "ready" ? runtimeTone ?? surfaceTone : surfaceTone;
  const statusIndicatorLabel = quotaStatus === "updated" || quotaStatus === "not_available" ? operationalLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${operationalLabel}`;
  const indicatorLabel = `${clientAuthWarning ? `${t("accounts.clientAuthWarning")} · ` : ""}${runtimeHint ? `${statusIndicatorLabel} · ${runtimeHint}` : statusIndicatorLabel}`;

  const updateParticipation = async (participate: boolean) => {
    if (membershipPending.current || participation.saving || (participate && !canAccountJoinPool(account))) return;
    membershipPending.current = true;
    try {
      const save = perform(
        `pool-${account.id}`,
        () => updatePoolMembership(mode, { accountIds: [account.id], sourceIds: [], inPool: participate }),
        "feedback.saved",
        { backgroundRefresh: true, uiLock: false },
      );
      participation.select(participate, () => save);
      await save;
    } finally {
      membershipPending.current = false;
    }
  };
  const recoverLocally = async () => {
    if (!await confirm(t("accounts.localRecoveryConfirm", { name: account.label }), {
      title: t("accounts.localRecovery"),
      confirmLabel: t("accounts.localRecoveryAction"),
      danger: true,
    })) return;
    await perform(`recover-account-${account.id}`, () => relayCommands.recoverLocalAccount(account.id), "feedback.accountRecoveredLocally", { backgroundRefresh: true });
  };
  const setEnabled = (enabled: boolean) => enabledFlag.select(enabled, () => perform(
    `enable-${account.id}`,
    () => relayCommands.setAccountEnabled(account.id, enabled),
    "feedback.saved",
    { backgroundRefresh: true, uiLock: false },
  ));
  const deleteAccount = async (permanently = false) => {
    const removed = await remove([account.id], permanently, `delete-${account.id}`, recoveryRequired);
    if (!removed) await refresh().catch(() => undefined);
  };

  return (
    <>
      <AccountCardLayout
        label={account.label}
        identityVisible={accountIdentitiesVisible}
        selected={selected}
        onToggleSelected={() => onToggleSelected(account.id)}
        icon={displayedErrorCode
          ? (
            <IconButton
              className="account-kind-icon account-status-button"
              data-status={indicatorTone}
              label={[accountErrorLabel(displayedErrorCode, t), runtimeHint].filter(Boolean).join(" · ")}
              icon={<UserRound aria-hidden />}
              onClick={() => onShowError(account)}
            />
          )
          : (
            <StatusIcon className="account-kind-icon" status={indicatorTone} label={indicatorLabel}>
              <UserRound aria-hidden />
            </StatusIcon>
          )}
        details={<div className="account-identity-meta"><AccountBadges planType={metadata.plan.subscription.planType} oauthClientKind={account.oauthClientKind} unknown={t("common.unknown")}
          planHint={metadata.plan.id !== account.id ? accountMetadataHint(metadata.plan, "plan", nowMs, t, i18n.language) : undefined}
          planStale={metadata.plan.id !== account.id && accountMetadataIsStale(metadata.plan, "plan", nowMs)}
          routeControl={<AccountRouteSelector account={account} />} /></div>}
        menu={
          <ActionMenu className="account-row-menu">
            {showNotes ? (
              <ActionMenuItem icon={<StickyNote aria-hidden />} onClick={() => setNotesOpen(true)}>
                {t("accounts.loginDetails")}
              </ActionMenuItem>
            ) : null}
            {recoveryRequired ? (
              <ActionMenuItem danger icon={<Power aria-hidden />} disabled={busy === `recover-account-${account.id}`} onClick={() => void recoverLocally()}>
                {t("accounts.localRecovery")}
              </ActionMenuItem>
            ) : null}
            <ActionMenuItem
              icon={<Network aria-hidden />}
              disabled={recoveryRequired}
              onClick={() => onProxy(account)}
            >
              {t("proxies.proxy")}: {proxyLabel}
            </ActionMenuItem>
            <ActionMenuItem
              icon={<Download aria-hidden />}
              disabled={!account.secretAvailable}
              onClick={() => onExport([account.id])}
            >
              {t("accounts.exportOne", { name: account.label })}
            </ActionMenuItem>
            {!recoveryRequired ? (
              <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void setEnabled(!accountEnabled); }}>
                {accountEnabled ? t("common.disable") : t("common.enable")}
              </ActionMenuItem>
            ) : null}
            <ActionMenuItem
              danger icon={<Trash2 aria-hidden />} disabled={Boolean(busy)}
              data-relay-context-action
              title={t("trash.deleteHint")}
              onClick={() => void deleteAccount()}
              onContextMenu={(event) => openDeleteMenu(event, () => void deleteAccount(true))}
            >
              {t("common.delete")}
            </ActionMenuItem>
          </ActionMenu>
        }
        actions={<>
          {recoveryRequired ? (
            <IconButton label={t("accounts.recoveryRequiredHint")} icon={<ShieldAlert aria-hidden />} disabled />
          ) : canAccountJoinPool(account) ? (
            <IconButton
              className={participates ? "danger" : ""}
              label={poolActionLabel}
              icon={participates ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />}
              disabled={participation.saving}
              onClick={() => void updateParticipation(!participates)}
            />
          ) : null}
          <IconButton
            label={t("accounts.refreshQuota")}
            icon={busy === `connection-account-quota-${account.id}`
              ? <Loader2 className="spin" aria-hidden />
              : <RefreshCw aria-hidden />}
            disabled={!canRefreshQuota || !account.secretAvailable || busy === `connection-account-quota-${account.id}`}
            onClick={() => void perform(
              `connection-account-quota-${account.id}`,
              () => refreshOneAccountQuota(mode, metadata.quota.id),
              "feedback.refreshed",
              { backgroundRefresh: true },
            )}
          />
          {mode === "local" && account.oauthClientKind !== "excel_bps" && account.oauthClientKind !== "siwc" ? (
            <IconButton
              label={t("accounts.launchAccount")}
              icon={<Play aria-hidden />}
              disabled={!canLaunch || busy === `launch-account-${account.id}`}
              title={recoveryRequired
                ? t("accounts.recoveryRequiredHint")
                : !account.secretAvailable
                  ? t("accounts.credentialsUnavailable")
                  : !account.enabled
                    ? t("common.disabled")
                  : !canLaunch
                    ? accountErrorLabel(errorCode ?? account.routingBlockReason ?? "auth_requires_reauth", t)
                    : t("accounts.launchAccount")}
              onClick={() => void activateCodexProfile(
                `launch-account-${account.id}`,
                () => relayCommands.launchCodexAccount(account.id),
                true,
              )}
            />
          ) : null}
        </>}
      >
        <div className="account-card-quota compact-quota-layout">
          <AccountQuotaPanel account={account} nowMs={nowMs} onReauthenticate={onReauthenticate} />
          {mode === "local" ? <ResetCreditsControl account={account} onCompleted={() => refresh()} /> : null}
        </div>
        <AccountProviderQuotaStrip account={account} />
        <AccountTokenExpiry account={account} nowMs={nowMs} />
        <AccountSubscriptionLine activeUntilMs={subscription.subscription.activeUntilMs} nowMs={nowMs}
          hint={subscription.id !== account.id ? accountMetadataHint(subscription, "plan", nowMs, t, i18n.language) : undefined} />
        {accountValueVisible ? <AccountValueStrip account={account} /> : null}
      </AccountCardLayout>
      {permanentDeleteMenu}
      {showNotes && notesOpen ? (
        <Dialog
          className="account-login-dialog"
          title={t("accounts.loginDetails")}
          onClose={() => setNotesOpen(false)}
        >
          <AccountLoginNotes accountId={account.id} />
        </Dialog>
      ) : null}
    </>
  );
}

export function AccountCardLayout({
  label,
  selected,
  onToggleSelected,
  icon,
  details,
  menu,
  actions,
  children,
  identityVisible = false,
  selectionDisabled = false,
  className = "",
  ...articleProps
}: {
  label: string;
  selected: boolean;
  onToggleSelected: () => void;
  icon: ReactNode;
  details: ReactNode;
  menu?: ReactNode;
  actions: ReactNode;
  children?: ReactNode;
  identityVisible?: boolean;
  selectionDisabled?: boolean;
} & HTMLAttributes<HTMLElement>) {
  const { t } = useTranslation();
  return <article {...articleProps} className={`account-card${selected ? " selected" : ""} ${className}`.trim()} role="listitem">
    <div className="account-card-main">
      {icon}
      <div className="account-identity">
        <strong className={identityVisible ? "revealed" : undefined} data-relay-tooltip={label}>{label}</strong>
        {details}
      </div>
      <div className="account-card-header-actions">
        {menu}
        <IconButton
          className="account-select-button"
          label={t(selected ? "accounts.deselect" : "accounts.select", { name: label })}
          icon={selected ? <Check aria-hidden /> : <Square aria-hidden />}
          aria-pressed={selected}
          disabled={selectionDisabled}
          onClick={onToggleSelected}
        />
      </div>
    </div>
    {children}
    <footer className="account-card-footer">
      <div className="account-card-actions">{actions}</div>
    </footer>
  </article>;
}
