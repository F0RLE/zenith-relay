import { useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Check,
  Clock3,
  Download,
  ListMinus,
  ListPlus,
  Loader2,
  Network,
  Play,
  Power,
  RefreshCw,
  StickyNote,
  Server,
  Square,
  Trash2,
  UserRound,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../../api/types";
import { accountQuotaRefreshState, canLaunchCodexAccount, currentAccountErrorCode, operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { refreshOneAccountQuota } from "../../accountQuotaRefresh";
import {
  AccountBadges,
  ActionMenu,
  ActionMenuItem,
  Button,
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
import { AccountCredentialDiagnostics } from "../../components/AccountCredentialDiagnostics";
import { useConnectionAccountRemoval } from "./accountRemoval";

type AccountCardProps = {
  account: AccountSummary;
  nowMs: number;
  selected: boolean;
  canManageProxies: boolean;
  canExport: boolean;
  canRefreshQuota: boolean;
  runtimeState: CandidateRuntimeSnapshot | undefined;
  onToggleSelected: (accountId: string) => void;
  onShowError: (account: AccountSummary) => void;
  onProxy: (account: AccountSummary) => void;
  onExport: (accountIds: string[]) => void;
  onReauthenticate: (account: AccountSummary) => void;
};

export function AccountCard({
  account,
  nowMs,
  selected,
  canManageProxies,
  canExport,
  canRefreshQuota,
  runtimeState,
  onToggleSelected,
  onShowError,
  onProxy,
  onExport,
  onReauthenticate,
}: AccountCardProps) {
  const { t } = useTranslation();
  const [notesOpen, setNotesOpen] = useState(false);
  const confirm = useConfirm();
  const membershipPending = useRef(false);
  const { mode, perform, activateCodexProfile, refresh, busy, accountIdentitiesVisible, accountValueVisible } = useRelayState();
  const { canTrash, remove, openDeleteMenu, permanentDeleteMenu } = useConnectionAccountRemoval();
  const participation = usePendingFlag(accountParticipates(account));
  const enabledFlag = usePendingFlag(account.enabled);
  const participates = participation.checked;
  const accountEnabled = enabledFlag.checked;
  const onServer = mode === "local" && Boolean(account.remoteLocation);
  const showNotes = mode === "local" && !onServer && account.secretAvailable;
  const errorCode = onServer
    ? account.lastErrorCode === "remote_missing" ? "remote_missing" : null
    : currentAccountErrorCode(account);
  const remoteMissing = onServer && errorCode === "remote_missing";
  const operationalStatus = account.operationalStatus;
  const operationalLabel = remoteMissing ? accountErrorLabel(errorCode, t) : onServer ? t("accounts.onServerHint") : t(`connections.status.${operationalStatus}`);
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
  const poolActionLabel = participates ? t("accounts.excludeFromPool") : t("accounts.includeInPool");
  const quotaStatus = accountQuotaRefreshState(account);
  const clientAuthWarning = account.clientAuthStatus === "login_required";
  const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
  const canLaunch = canLaunchCodexAccount(account) && !onServer;
  const indicatorTone = onServer
    ? "info"
    : operationalStatus === "unavailable" || operationalStatus === "disabled"
      ? operationalStatusTone(operationalStatus)
      : quotaStatus === "refreshing"
        ? "disabled"
        : quotaStatus === "failed" || quotaStatus === "requires_reauth"
          ? "error"
          : clientAuthWarning
            ? "warning"
            : quotaStatus === "pending"
              ? "disabled"
              : runtimeTone ?? operationalStatusTone(operationalStatus);
  const statusIndicatorLabel = quotaStatus === "updated" ? operationalLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${operationalLabel}`;
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
  const returnToComputer = async () => {
    if (!await confirm(t("accounts.returnToComputerConfirm", { name: account.label }), {
      title: t("accounts.returnToComputer"),
      confirmLabel: t("accounts.returnToComputerAction"),
    })) return;
    await perform(`return-account-${account.id}`, () => relayCommands.returnAccountToLocal(account.id), "feedback.accountReturnedToComputer", { backgroundRefresh: true });
  };
  const recoverLocally = async () => {
    if (!await confirm(t("accounts.forceActivateLocalConfirm", { name: account.label }), {
      title: t("accounts.forceActivateLocal"),
      confirmLabel: t("accounts.forceActivateLocalAction"),
      danger: true,
    })) return;
    await perform(`recover-account-${account.id}`, () => relayCommands.forceActivateRemoteAccountLocally(account.id), "feedback.accountRecoveredLocally", { backgroundRefresh: true });
  };
  const setEnabled = (enabled: boolean) => enabledFlag.select(enabled, () => perform(
    `enable-${account.id}`,
    () => mode === "local"
      ? relayCommands.setAccountEnabled(account.id, enabled)
      : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled }),
    "feedback.saved",
    { backgroundRefresh: true, uiLock: false },
  ));
  const deleteAccount = async (permanently = false) => {
    const removed = await remove([account.id], permanently, `delete-${account.id}`, onServer);
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
              data-status="error"
              label={accountErrorLabel(displayedErrorCode, t)}
              icon={<UserRound aria-hidden />}
              onClick={() => onShowError(account)}
            />
          )
          : (
            <StatusIcon className="account-kind-icon" status={indicatorTone} label={indicatorLabel}>
              <UserRound aria-hidden />
            </StatusIcon>
          )}
        details={<div className="account-identity-meta"><AccountBadges planType={account.subscription.planType} oauthClientKind={account.oauthClientKind} unknown={t("common.unknown")} /></div>}
        menu={
          <ActionMenu className="account-row-menu">
            {showNotes ? (
              <ActionMenuItem icon={<StickyNote aria-hidden />} onClick={() => setNotesOpen(true)}>
                {t("accounts.loginDetails")}
              </ActionMenuItem>
            ) : null}
            {onServer ? (
              <ActionMenuItem icon={<Download aria-hidden />} disabled={busy === `return-account-${account.id}`} onClick={() => void returnToComputer()}>
                {t("accounts.returnToComputer")}
              </ActionMenuItem>
            ) : null}
            {onServer ? (
              <ActionMenuItem danger icon={<Power aria-hidden />} disabled={busy === `recover-account-${account.id}`} onClick={() => void recoverLocally()}>
                {t("accounts.forceActivateLocal")}
              </ActionMenuItem>
            ) : null}
            <ActionMenuItem
              icon={<Network aria-hidden />}
              disabled={onServer || !canManageProxies}
              onClick={() => onProxy(account)}
            >
              {t("proxies.proxy")}: {proxyLabel}
            </ActionMenuItem>
            <ActionMenuItem
              icon={<Download aria-hidden />}
              disabled={!canExport || !account.secretAvailable}
              onClick={() => onExport([account.id])}
            >
              {t("accounts.exportOne", { name: account.label })}
            </ActionMenuItem>
            {!onServer ? (
              <ActionMenuItem icon={<Power aria-hidden />} onClick={() => { void setEnabled(!accountEnabled); }}>
                {accountEnabled ? t("common.disable") : t("common.enable")}
              </ActionMenuItem>
            ) : null}
            <ActionMenuItem
              danger icon={<Trash2 aria-hidden />} disabled={Boolean(busy)}
              data-relay-context-action
              title={t(canTrash ? "trash.deleteHint" : "trash.serverUpdateRequired")}
              onClick={() => void deleteAccount()}
              onContextMenu={(event) => openDeleteMenu(event, () => void deleteAccount(true))}
            >
              {t("common.delete")}
            </ActionMenuItem>
          </ActionMenu>
        }
        actions={<>
          {onServer ? (
            <IconButton label={t("accounts.onServerHint")} icon={<Server aria-hidden />} disabled />
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
              () => refreshOneAccountQuota(mode, account.id),
              "feedback.refreshed",
              { backgroundRefresh: true },
            )}
          />
          {mode === "local" ? (
            <IconButton
              label={t("accounts.launchAccount")}
              icon={<Play aria-hidden />}
              disabled={!canLaunch || busy === `launch-account-${account.id}`}
              title={onServer
                ? t("accounts.onServerHint")
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
        <AccountCredentialDiagnostics account={account} />
        <AccountSubscriptionLine activeUntilMs={account.subscription.activeUntilMs} nowMs={nowMs} />
        {runtimeHint ? (
          <div className="account-runtime-line" data-warning={modelRetries.length > 0}>
            <Clock3 aria-hidden />
            <span>{runtimeHint}</span>
          </div>
        ) : null}
        {accountValueVisible ? <AccountValueStrip account={account} /> : null}
      </AccountCardLayout>
      {permanentDeleteMenu}
      {showNotes && notesOpen ? (
        <Dialog
          className="account-login-dialog"
          title={t("accounts.loginDetails")}
          onClose={() => setNotesOpen(false)}
          footer={<Button variant="secondary" onClick={() => setNotesOpen(false)}>{t("common.close")}</Button>}
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
}) {
  const { t } = useTranslation();
  return <article className={`account-card${selected ? " selected" : ""}`} role="listitem">
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
