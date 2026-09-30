import { Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import { Button, Dialog, accountErrorLabel, copyText } from "../../components/Ui";

export function AccountErrorDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const code = currentAccountErrorCode(account) ?? "unknown";
  const authState = account.authState.state;
  const observedAtMs = account.quota.error?.occurredAtMs ?? null;
  const details = JSON.stringify({
    code,
    message: accountErrorLabel(code, t),
    observed_at: observedAtMs ? new Date(observedAtMs).toISOString() : null,
    account: account.identityHint || account.label,
    health: account.health,
    auth_state: authState,
    subscription_status: account.subscription.status,
  }, null, 2);
  return (
    <Dialog
      title={t("accounts.errorDetailsTitle")}
      onClose={onClose}
      footer={(
        <>
          <Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => void copyText(details)}>
            {t("common.copy")}
          </Button>
          <Button variant="primary" onClick={onClose}>{t("common.close")}</Button>
        </>
      )}
    >
      <div className="config-preview account-error-json">
        <pre><code>{details}</code></pre>
      </div>
      <p className="form-note">{t("accounts.errorDetailsHint")}</p>
    </Dialog>
  );
}
