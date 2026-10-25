import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../../api/types";
import { currentAccountErrorCode } from "../../accountStatus";
import { Button, Dialog, accountErrorLabel, copyText } from "../../components/Ui";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { accountErrorContext } from "./accountErrorContext";

export function AccountErrorDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const [copied, showCopied, clearCopied] = useTransientFlag(1_500);
  const code = currentAccountErrorCode(account) ?? "unknown";
  const authState = account.authState.state;
  const message = accountErrorLabel(code, t);
  const errorDetailsJson = JSON.stringify({
    code,
    message,
    ...accountErrorContext(account, code),
    connection_kind: account.oauthClientKind ?? "codex",
    health: account.health,
    auth_state: authState,
    subscription_status: account.subscription.status,
    ...(/timeout|transport|network|connect|proxy/i.test(code)
      ? { recovery: t("accounts.connectionErrorHint") }
      : {}),
  }, null, 2);
  const copyError = async () => {
    try {
      await copyText(errorDetailsJson);
      showCopied();
    } catch {
      clearCopied();
    }
  };
  return (
    <Dialog
      title={t("accounts.errorDetailsTitle")}
      onClose={onClose}
      className="account-error-dialog"
      footer={(
        <>
          <Button variant="secondary" icon={copied ? <Check aria-hidden /> : <Copy aria-hidden />} onClick={() => void copyError()}>
            {copied ? t("feedback.copied") : t("common.copy")}
          </Button>
          <Button variant="primary" onClick={onClose}>{t("common.close")}</Button>
        </>
      )}
    >
      <div className="config-preview account-error-json">
        <pre><code>{errorDetailsJson}</code></pre>
      </div>
    </Dialog>
  );
}
