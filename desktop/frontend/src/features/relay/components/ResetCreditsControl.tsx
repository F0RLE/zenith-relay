import { useEffect, useState } from "react";
import { Loader2, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary } from "../api/types";
import { accountResetCreditsSource } from "../accountRoutes";
import { sanitizeFeedbackError } from "../state/feedback";
import { useRelayState } from "../state/RelayStateProvider";
import { ResetQuotaDialog } from "./ResetQuotaDialog";

type ResetCreditsControlProps = {
  account: AccountSummary;
  onCompleted?: () => Promise<void> | void;
};

function canResetQuota(account: AccountSummary) {
  return account.authState.state === "active" && account.secretAvailable
    && account.routingBlockReason !== "reauth_required" && !account.recoveryRequired;
}

export function ResetCreditsControl({ account, onCompleted }: ResetCreditsControlProps) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const [confirming, setConfirming] = useState(false);
  const [consuming, setConsuming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const feedback = error ? <span className="reset-credits-inline-error" role="alert">{error}</span> : null;

  const source = accountResetCreditsSource(account, runtime?.accounts ?? []);
  const availableFromQuota = source?.quota.resetCreditsAvailable;
  const available = canResetQuota(account) && source && canResetQuota(source)
    && availableFromQuota != null && availableFromQuota > 0;
  useEffect(() => { setConfirming(false); }, [Boolean(available), source?.id]);
  if (!available || !source) {
    return feedback;
  }

  const linked = source.id !== account.id;
  const client = source.oauthClientKind === "excel_bps" ? "Excel / BPS" : "Codex";
  const title = t(linked ? "accounts.resetCreditsLinkedTitle" : "accounts.resetCreditsTitle", { client });

  const errorMessage = (errorValue: unknown) => {
    return sanitizeFeedbackError(errorValue, "reset_credits_failed", t("accounts.resetCreditsLoadFailed")).message;
  };

  const consume = async () => {
    if (consuming) return;
    setConfirming(false);
    setConsuming(true);
    setError(null);
    try {
      const resetCreditResult = await relayCommands.consumeResetCredit(source.id);
      // Preserve partial success even when refresh removes the last-credit control.
      if (resetCreditResult.refreshError) {
        setError(t("accounts.resetCreditsRefreshFailed", { error: errorMessage(resetCreditResult.refreshError) }));
      }
      try {
        await onCompleted?.();
      } catch (refreshError) {
        setError(t("accounts.resetCreditsRefreshFailed", { error: errorMessage(refreshError) }));
        return;
      }
    } catch (resetError) {
      setError(t("accounts.resetCreditsFailed", { error: errorMessage(resetError) }));
    } finally {
      setConsuming(false);
    }
  };

  return <>
    <button
      type="button"
      className="reset-credits-control"
      data-available="true"
      disabled={consuming}
      aria-label={`${t("accounts.resetCreditsAvailable", { count: availableFromQuota })} · ${title}`}
      data-relay-tooltip={title}
      onClick={() => setConfirming(true)}
    >
      <span className="reset-credits-copy">
        {consuming ? <Loader2 className="reset-credits-status-icon spin" aria-hidden /> : <RotateCcw className="reset-credits-status-icon" aria-hidden />}
        <span className="reset-credits-action-label">{t("accounts.resetCreditsExecute")}</span>
        <span className="reset-credits-count" aria-hidden>{availableFromQuota}</span>
      </span>
    </button>
    {confirming ? <ResetQuotaDialog onClose={() => setConfirming(false)} onConfirm={() => void consume()} /> : null}
    {feedback}
  </>;
}
