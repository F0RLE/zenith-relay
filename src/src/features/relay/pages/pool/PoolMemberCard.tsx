import { Clock3, Cloud, ListMinus, Loader2, Pencil, RefreshCw, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary, CandidateRuntimeSnapshot, PoolRoutingMode } from "../../api/types";
import { accountQuotaRefreshState, currentAccountErrorCode, operationalStatusTone, transientCandidateTone } from "../../accountStatus";
import { refreshOneAccountQuota } from "../../accountQuotaRefresh";
import { ResetCreditsControl } from "../../components/ResetCreditsControl";
import { AccountPlanBadge, IconButton, StatusIcon, accountErrorLabel } from "../../components/Ui";
import { AccountValueStrip } from "../../components/AccountValueStrip";
import { AccountProviderQuotaStrip } from "../../components/AccountProviderQuotaStrip";
import { AccountQuotaPanel } from "../../components/AccountQuotaPanel";
import { AccountSubscriptionLine } from "../../components/AccountSubscriptionLine";
import { formatDetailedRemainingTime } from "../../quotaFormatting";
import { activeRequestCount, upcomingModelRetries } from "../../routingOrder";
import type { PoolMember } from "../../poolHelpers";
import { SourceStatsPanel } from "../../components/SourceStatsPanel";
import type { SourceStatsState } from "../../sourceStatsModel";
import { useRelayState } from "../../state/relayStateContext";

type PoolMemberCardProps = {
  member: PoolMember;
  nowMs: number;
  runtimeState: CandidateRuntimeSnapshot | undefined;
  selected: boolean;
  isNext: boolean;
  isLastUsed: boolean;
  sourceState: SourceStatsState | undefined;
  rotationMode: PoolRoutingMode | null;
  canRefreshQuota: boolean;
  onShowError: (account: AccountSummary) => void;
  onEdit: () => void;
  onRemove: () => void;
  onConfirmRemove: () => void;
  onRefreshSource: () => void;
  onReauthenticate: (account: AccountSummary) => void;
};

export function PoolMemberCard({
  member,
  nowMs,
  runtimeState,
  selected,
  isNext,
  isLastUsed,
  sourceState,
  rotationMode,
  canRefreshQuota,
  onShowError,
  onEdit,
  onRemove,
  onConfirmRemove,
  onRefreshSource,
  onReauthenticate,
}: PoolMemberCardProps) {
  const { t } = useTranslation();
  const { mode, perform, refresh, busy, accountValueVisible, codexPoolOauthSelection } = useRelayState();
  const statusKey = member.operationalStatus;
  const statusTone = operationalStatusTone(statusKey);
  const quotaStatus = member.kind === "account" ? accountQuotaRefreshState(member) : "updated";
  const errorCode = member.kind === "account" ? currentAccountErrorCode(member) : null;
  const displayedErrorCode = quotaStatus === "refreshing" ? null : errorCode;
  const codexInterface = member.kind === "account" && codexPoolOauthSelection === member.id;
  const identity = member.kind === "source" ? member.name : member.identityHint || member.label;
  const detail = member.kind === "source" ? `${member.wireApi} · ${member.baseUrl}` : member.label;
  const isCurrent = activeRequestCount(runtimeState) > 0;
  const modelRetries = upcomingModelRetries(runtimeState, nowMs);
  const firstModelRetry = modelRetries[0];
  const modelRetryHint = firstModelRetry
    ? t("pool.modelRetryAt", {
      models: modelRetries.map((retry) => retry.model).join(", "),
      time: formatDetailedRemainingTime(firstModelRetry.retryAtMs, nowMs, t),
    })
    : null;
  const memberErrorCode = member.kind === "source" ? member.lastErrorCode?.trim() : errorCode;
  const visibleMemberErrorCode = member.kind === "source" ? memberErrorCode : displayedErrorCode;
  const runtimeTone = statusKey === "rotation"
    ? member.kind === "source"
      ? transientCandidateTone(runtimeState, nowMs, true)
      : modelRetries.length > 0
        ? "warning"
        : transientCandidateTone(runtimeState, nowMs, false)
    : null;
  const indicatorTone = visibleMemberErrorCode
    ? "error"
    : statusKey === "unavailable" || statusKey === "disabled"
    ? statusTone
    : quotaStatus === "refreshing"
      ? "disabled"
      : quotaStatus === "failed" || quotaStatus === "requires_reauth"
        ? "error"
        : quotaStatus === "pending"
          ? "disabled"
          : runtimeTone ?? statusTone;
  const runtimeHint = runtimeState?.halfOpen
    ? t("pool.recoveryProbe")
    : modelRetryHint
      ? modelRetryHint
      : member.kind === "source" && runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
      ? t("pool.retryAt", { time: formatDetailedRemainingTime(runtimeState.nextRetryAtMs, nowMs, t) })
      : undefined;
  const activeRequests = activeRequestCount(runtimeState);
  const name = member.kind === "source" ? member.name : member.label;
  const editLabel = `${t("pool.editMember")}: ${name}`;
  const removeLabel = `${t("pool.removeMember")}: ${name}`;
  const removing = busy === `pool-remove-${member.id}`;
  const statusLabel = t(`pool.memberStatus.${statusKey}`);
  const indicatorLabel = visibleMemberErrorCode
    ? member.kind === "account" ? accountErrorLabel(visibleMemberErrorCode, t) : t("pool.runtimeError", { code: visibleMemberErrorCode })
    : quotaStatus === "updated" ? statusLabel : `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${statusLabel}`;
  const indicatorHint = member.kind === "source"
    ? [indicatorLabel, runtimeHint].filter(Boolean).join(" · ")
    : [runtimeHint, indicatorLabel].filter(Boolean).join(" · ");

  return (
    <article
      className={`pool-member-card${selected ? " selected" : ""}${isCurrent ? " current" : ""}${isNext ? " next" : ""}${isLastUsed ? " last-used" : ""}`}
      role="listitem"
      data-member-label={name}
      data-current={isCurrent ? "true" : "false"}
      data-next={isNext ? "true" : "false"}
      data-last-used={isLastUsed ? "true" : "false"}
      data-member-kind={member.kind}
    >
      <header className="pool-member-card-header">
        {member.kind === "account" && displayedErrorCode
          ? (
            <IconButton
              className="pool-member-kind-icon"
              data-status="error"
              label={indicatorLabel}
              icon={<UserRound aria-hidden />}
              onClick={() => onShowError(member)}
            />
          )
          : (
            <StatusIcon
              className="pool-member-kind-icon"
              status={indicatorTone}
              label={[indicatorHint, codexInterface ? t("pool.codexInterfaceHint") : null].filter(Boolean).join(" · ")}
              showTooltip={!(member.kind === "source" && visibleMemberErrorCode)}
            >
              {member.kind === "source" ? <Cloud aria-hidden /> : <UserRound aria-hidden />}
            </StatusIcon>
          )}
        <div className="pool-member-identity">
          <strong
            className="pool-member-name"
            data-relay-tooltip={member.kind === "account" && identity !== detail ? `${identity} · ${detail}` : identity}
          >
            {identity}
          </strong>
          <div className="pool-member-meta">
            {member.kind === "account"
              ? <AccountPlanBadge planType={member.subscription.planType} unknown={t("common.unknown")} />
              : <small data-relay-tooltip={detail}>{detail}</small>}
          </div>
        </div>
      </header>
      <div className={`pool-member-card-quota${member.kind === "account" ? " compact-quota-layout" : ""}`}>
        {member.kind === "account"
          ? <AccountQuotaPanel account={member} nowMs={nowMs} onReauthenticate={onReauthenticate} />
          : <SourceStatsPanel source={member} {...(sourceState ? { state: sourceState } : {})} />}
        {mode === "local" && member.kind === "account" ? <ResetCreditsControl account={member} onCompleted={() => refresh()} /> : null}
      </div>
      {member.kind === "account" ? <AccountProviderQuotaStrip account={member} /> : null}
      {member.kind === "account" ? (
        <>
          <AccountSubscriptionLine activeUntilMs={member.subscription.activeUntilMs} nowMs={nowMs} />
          {runtimeHint ? (
            <div className="account-runtime-line" data-warning={modelRetries.length > 0}>
              <Clock3 aria-hidden /><span>{runtimeHint}</span>
            </div>
          ) : null}
        </>
      ) : (
        <div className="pool-member-context" data-kind="source">
          <div className="pool-member-runtime-meta">
            {rotationMode ? <div><span>{t("pool.operationMode")}</span><strong>{t(`pool.rotationModes.${rotationMode === "automatic" ? "automatic" : "manual"}`)}</strong></div> : null}
            <div><span>{t("pool.activeRequestCount")}</span><strong>{activeRequests}</strong></div>
          </div>
        </div>
      )}
      {member.kind === "account" && accountValueVisible ? <AccountValueStrip account={member} /> : null}
      <footer className="pool-member-card-footer" data-kind={member.kind}>
        <div className="pool-member-actions">
          <IconButton
            className="danger"
            data-relay-context-action
            label={removeLabel}
            icon={removing ? <Loader2 className="spin" aria-hidden /> : <ListMinus aria-hidden />}
            disabled={removing}
            onClick={onConfirmRemove}
            onContextMenu={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onRemove();
            }}
          />
          {member.kind === "source" ? (
            <IconButton
              label={t("pool.refreshSourceStats")}
              icon={sourceState?.loading ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />}
              disabled={!member.secretAvailable || sourceState?.loading}
              onClick={onRefreshSource}
            />
          ) : null}
          {member.kind === "account" ? (
            <IconButton
              label={t("accounts.refreshQuota")}
              icon={busy === `pool-account-quota-${member.id}`
                ? <Loader2 className="spin" aria-hidden />
                : <RefreshCw aria-hidden />}
              disabled={!canRefreshQuota || !member.secretAvailable || busy === `pool-account-quota-${member.id}`}
              onClick={() => void perform(
                `pool-account-quota-${member.id}`,
                () => refreshOneAccountQuota(mode, member.id),
                "feedback.refreshed",
                { backgroundRefresh: true },
              )}
            />
          ) : null}
          <IconButton label={editLabel} icon={<Pencil aria-hidden />} aria-haspopup="dialog" onClick={onEdit} />
        </div>
      </footer>
    </article>
  );
}
