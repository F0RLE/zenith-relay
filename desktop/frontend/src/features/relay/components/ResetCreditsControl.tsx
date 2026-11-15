import { useState } from "react";
import { Loader2, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { AccountSummary } from "../api/types";
import { accountResetCreditsSource } from "../accountRoutes";
import { sanitizeFeedbackError } from "../state/feedback";
import { useRelayState } from "../state/RelayStateProvider";
import { useConfirm } from "./Ui";

type ResetCreditsControlProps = {
  account: AccountSummary;
  onCompleted?: () => Promise<void> | void;
};

export function ResetCreditsControl({ account, onCompleted }: ResetCreditsControlProps) {
  const { t } = useTranslation();
  const { runtime } = useRelayState();
  const confirm = useConfirm();
  const [consuming, setConsuming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const source = accountResetCreditsSource(account, runtime?.accounts ?? []);
  const availableFromQuota = source?.quota.resetCreditsAvailable;
  if (account.authState.state !== "active" || !account.secretAvailable
    || account.routingBlockReason === "reauth_required" || account.recoveryRequired
    || !source || source.authState.state !== "active" || !source.secretAvailable
    || source.routingBlockReason === "reauth_required" || source.recoveryRequired
    || availableFromQuota == null || availableFromQuota <= 0) {
    return error ? <span className="reset-credits-inline-error" role="alert">{error}</span> : null;
  }

  const linked = source.id !== account.id;
  const client = source.oauthClientKind === "excel_bps" ? "Excel / BPS" : "Codex";
  const title = t(linked ? "accounts.resetCreditsLinkedTitle" : "accounts.resetCreditsTitle", { client });

  const errorMessage = (errorValue: unknown) => {
    return sanitizeFeedbackError(errorValue, "reset_credits_failed", t("accounts.resetCreditsLoadFailed")).message;
  };

  const consume = async () => {
    if (consuming) return;
    const accepted = await confirm(t(linked ? "accounts.resetCreditsLinkedConfirm" : "accounts.resetCreditsConfirm", { client }), {
      title,
      cancelLabel: t("accounts.resetCreditsConfirmNo"),
      confirmLabel: t("accounts.resetCreditsConfirmYes"),
    });
    if (!accepted) return;
    setConsuming(true);
    setError(null);
    try {
      const resetCreditResult = await relayCommands.consumeResetCredit(source.id);
      // Set the partial-success diagnostic before refreshing the account. The
      // refresh can consume the last available credit and temporarily remove
      // the button from the card; the error must survive that state change.
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
