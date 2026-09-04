import { useState } from "react";
import { Check, Clock3, Copy, ExternalLink, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { OAuthFlow } from "../../api/types";
import { Button, Dialog, copyText } from "../../components/Ui";
import { secondsUntil, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { useRelayState } from "../../state/RelayStateProvider";
import { useProxyPool } from "./ProxyDialogs";
export function OAuthDialog({ flow, onCancel }: { flow: OAuthFlow; onCancel: () => Promise<void> }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [reopenAt, setReopenAt] = useState(0);
  const [linkCopied, showLinkCopied] = useTransientFlag(1_500);
  const now = useRelativeTimeClock([flow.expiresAtMs, reopenAt || null]);
  const secondsRemaining = secondsUntil(flow.expiresAtMs, now);
  const reopenIn = secondsUntil(reopenAt, now);
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const flowFailed = flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed";
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const reopen = async () => {
    const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId));
    if (opened) setReopenAt(Date.now() + 3_000);
  };
  const copyLink = async () => {
    await copyText(flow.authorizationUrl);
    showLinkCopied();
  };
  return <Dialog
    title={t("accounts.signIn")}
    onClose={() => void onCancel()}
    footer={<Button variant="secondary" busy={busy === "oauth-cancel"} onClick={() => void onCancel()}>{t("common.cancel")}</Button>}
  >
    <div className="relay-form oauth-waiting">
      <div className="oauth-waiting-status"><Loader2 className="spin" aria-hidden /><div><strong>{t(callbackReceived ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong><p>{t("accounts.waitingForSignInHint")}</p></div></div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="oauth-expiry" role="timer"><Clock3 aria-hidden /><span>{t("accounts.oauthRemaining")}</span><strong>{formatCountdown(secondsRemaining)}</strong></div>
      <div className="oauth-link-actions">
        <Button variant="primary" icon={<ExternalLink aria-hidden />} busy={busy === "oauth-reopen"} disabled={flowUnavailable || reopenIn > 0} onClick={() => void reopen()}>{reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}</Button>
        <Button variant="secondary" icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />} disabled={flowUnavailable} onClick={() => void copyLink()}>{t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}</Button>
      </div>
    </div>
  </Dialog>;
}

export function OAuthAccountSetupDialog({ accountId, onClose }: { accountId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const [addToPool, setAddToPool] = useState(true);
  const [assignProxy, setAssignProxy] = useState(false);
  const account = runtime?.accounts.find((item) => item.id === accountId);
  const apply = async () => {
    if (!addToPool && !assignProxy) {
      onClose();
      return;
    }
    const ok = await perform("oauth-setup", async () => {
      if (addToPool) await relayCommands.setPoolMembership([accountId], [], true);
      if (assignProxy) await relayCommands.assignAutomaticProxies([accountId]);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.accountAdded")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("accounts.configureLater")}</Button><Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}>{t("common.done")}</Button></>}><div className="relay-form oauth-account-setup"><div className="oauth-account-added"><Check aria-hidden /><div><strong>{account?.identityHint ?? t("accounts.accountReady")}</strong><p>{t("accounts.accountAddedHint")}</p></div></div><div className="post-import-options"><label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addAccountToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label><label><input type="checkbox" checked={assignProxy} disabled={!pool || pool.total === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignStoredAfterAdd")}</strong><small>{pool ? t(pool.total ? "proxies.storedAvailable" : "proxies.noStored", { count: pool.total }) : t("common.loading")}</small></span></label></div></div></Dialog>;
}

function formatCountdown(seconds: number) {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}
