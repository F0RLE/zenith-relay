import { useState } from "react";
import { Loader2, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary } from "../api/types";
import { sanitizeFeedbackError } from "../state/feedback";
import { useConfirm } from "./Ui";

type ResetCreditsControlProps = {
  account: AccountSummary;
  onCompleted?: () => Promise<void> | void;
};

export function ResetCreditsControl({ account, onCompleted }: ResetCreditsControlProps) {
  const { t } = useTranslation();
  const confirm = useConfirm();
  const [consuming, setConsuming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const availableFromQuota = account.quota.resetCreditsAvailable;
  if (account.remoteLocation || availableFromQuota == null || availableFromQuota <= 0) {
    return error ? <span className="reset-credits-inline-error" role="alert">{error}</span> : null;
  }

  const errorMessage = (value: unknown) => {
    return sanitizeFeedbackError(value, "reset_credits_failed", t("accounts.resetCreditsLoadFailed")).message;
  };

  const consume = async () => {
    if (consuming) return;
    const accepted = await confirm(t("accounts.resetCreditsConfirm"), {
      title: t("accounts.resetCreditsTitle"),
      cancelLabel: t("accounts.resetCreditsConfirmNo"),
      confirmLabel: t("accounts.resetCreditsConfirmYes"),
    });
    if (!accepted) return;
    setConsuming(true);
    setError(null);
    try {
      const result = await relayCommands.consumeResetCredit(account.id);
      // Set the partial-success diagnostic before refreshing the account. The
      // refresh can consume the last available credit and temporarily remove
      // the button from the card; the error must survive that state change.
      if (result.refreshError) {
        setError(t("accounts.resetCreditsRefreshFailed", { error: errorMessage(result.refreshError) }));
      }
      try {
        await onCompleted?.();
      } catch (value) {
        setError(t("accounts.resetCreditsRefreshFailed", { error: errorMessage(value) }));
        return;
      }
    } catch (value) {
      setError(t("accounts.resetCreditsFailed", { error: errorMessage(value) }));
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
      aria-label={`${t("accounts.resetCreditsAvailable", { count: availableFromQuota })} · ${t("accounts.resetCreditsExecute")}`}
      data-relay-tooltip={t("accounts.resetCreditsTitle")}
      onClick={() => void consume()}
    >
      <span className="reset-credits-copy">
        {consuming ? <Loader2 className="reset-credits-status-icon spin" aria-hidden /> : <RotateCcw className="reset-credits-status-icon" aria-hidden />}
        <span className="reset-credits-action-label">{t("accounts.resetCreditsExecute")}</span>
        <span className="reset-credits-count" aria-hidden>{availableFromQuota}</span>
      </span>
    </button>
    {error ? <span className="reset-credits-inline-error" role="alert">{error}</span> : null}
  </>;
}
