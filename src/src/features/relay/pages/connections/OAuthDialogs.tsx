import { useEffect, useState } from "react";
import { Check, ChevronDown, Clock3, Copy, ExternalLink, Loader2, Lock, Network } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { OAuthFlow, ProxyPoolEntry } from "../../api/types";
import { Button, Dialog, IconButton, copyText } from "../../components/Ui";
import { AccountLoginNotes } from "../../components/AccountLoginNotes";
import { secondsUntil, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { useRelayState } from "../../state/RelayStateProvider";
import { isHttpProxyEndpoint, rememberedSignInProxyId } from "./signInProxyPreference";
import { useProxyPool } from "./useProxyPool";
export function OAuthDialog({ flow, onCancel, onUseProxy }: { flow: OAuthFlow; onCancel: () => Promise<void>; onUseProxy?: ((proxyId: string) => void) | undefined }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [reopenAt, setReopenAt] = useState(0);
  const [notesOpen, setNotesOpen] = useState(false);
  const [proxyOpen, setProxyOpen] = useState(false);
  const [linkCopied, showLinkCopied] = useTransientFlag(1_500);
  const now = useRelativeTimeClock([flow.expiresAtMs, reopenAt || null]);
  const secondsRemaining = secondsUntil(flow.expiresAtMs, now);
  const reopenIn = secondsUntil(reopenAt, now);
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const flowFailed = flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed";
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const closeLocked = busy === "oauth-complete" || busy === "oauth-cancel";
  const reauthentication = Boolean(flow.targetAccountId);
  const proxies = useProxyPool(!reauthentication && Boolean(onUseProxy));
  const requestClose = () => {
    if (closeLocked) return;
    void onCancel();
  };
  const reopen = async () => {
    const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId), undefined, { backgroundRefresh: true });
    if (opened) setReopenAt(Date.now() + 3_000);
  };
  const copyLink = async () => {
    await copyText(flow.authorizationUrl);
    showLinkCopied();
  };
  const chooseProxy = () => {
    if (!onUseProxy || flowUnavailable) return;
    setProxyOpen(true);
  };
  const repeatProxy = () => {
    if (!onUseProxy || flowUnavailable) return;
    const remembered = rememberedSignInProxyId();
    const entry = proxies.pool?.entries.find((item) => item.id === remembered);
    if (entry && isHttpProxyEndpoint(entry.endpoint)) {
      onUseProxy(entry.id);
      return;
    }
    setProxyOpen(true);
  };
  return <>
  <Dialog
    className="oauth-sign-in-dialog"
    title={t("accounts.signIn")}
    onClose={requestClose}
    footer={<Button variant="secondary" busy={busy === "oauth-cancel"} disabled={closeLocked} onClick={requestClose}>{t("common.cancel")}</Button>}
  >
    <div className="relay-form oauth-waiting">
      <div className="oauth-waiting-status">
        <Loader2 className="spin" aria-hidden />
        <div>
          <strong>{t(callbackReceived ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong>
          <p>{t("accounts.waitingForSignInHint")}</p>
        </div>
      </div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="oauth-expiry" role="timer"><Clock3 aria-hidden /><span>{t("accounts.oauthRemaining")}</span><strong>{formatCountdown(secondsRemaining)}</strong></div>
      <div className="oauth-link-actions">
        {onUseProxy && !reauthentication ? (
          <div className={flowUnavailable ? "oauth-open-with-lock is-disabled" : "oauth-open-with-lock"}>
            <Button
              variant="primary"
              icon={<ExternalLink aria-hidden />}
              busy={busy === "oauth-reopen"}
              disabled={flowUnavailable || reopenIn > 0}
              onClick={() => void reopen()}
            >
              {reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}
            </Button>
            <IconButton
              className="oauth-open-lock"
              label={t("accounts.signInWithProxy")}
              title={t("accounts.signInWithProxyHint")}
              icon={<Lock aria-hidden />}
              disabled={flowUnavailable || busy === "oauth-start"}
              onClick={chooseProxy}
              onContextMenu={(event) => {
                event.preventDefault();
                repeatProxy();
              }}
            />
          </div>
        ) : (
          <Button
            variant="primary"
            icon={<ExternalLink aria-hidden />}
            busy={busy === "oauth-reopen"}
            disabled={flowUnavailable || reopenIn > 0}
            onClick={() => void reopen()}
          >
            {reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}
          </Button>
        )}
        <Button
          variant="secondary"
          icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />}
          disabled={flowUnavailable}
          onClick={() => void copyLink()}
        >
          {t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}
        </Button>
      </div>
      {reauthentication ? <section className="oauth-notes-card">
        <button type="button" className="oauth-notes-toggle" aria-expanded={notesOpen} onClick={() => setNotesOpen((open) => !open)}>
          <span>{t("accounts.loginDetails")}</span>
          <ChevronDown aria-hidden />
        </button>
        {notesOpen && flow.targetAccountId ? <AccountLoginNotes accountId={flow.targetAccountId} /> : null}
      </section> : null}
    </div>
  </Dialog>
  {proxyOpen && onUseProxy ? (
    <SignInProxyDialog
      entries={proxies.pool?.entries ?? []}
      loading={!proxies.pool && !proxies.failed}
      failed={proxies.failed}
      initialProxyId={rememberedSignInProxyId()}
      onClose={() => setProxyOpen(false)}
      onConfirm={(proxyId) => {
        setProxyOpen(false);
        onUseProxy(proxyId);
      }}
    />
  ) : null}
  </>;
}

export function OAuthAccountSetupDialog({ accountId, preserveProxy = false, onClose }: { accountId: string; preserveProxy?: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const [addToPool, setAddToPool] = useState(false);
  const [assignProxy, setAssignProxy] = useState(false);
  const account = runtime?.accounts.find((item) => item.id === accountId);
  const hasAccountProxy = preserveProxy || account?.proxyMode === "account";
  const closeLocked = busy === "oauth-setup";
  const requestClose = () => {
    if (closeLocked) return;
    onClose();
  };
  const apply = async () => {
    const assignStoredProxy = assignProxy && !hasAccountProxy;
    if (!addToPool && !assignStoredProxy) {
      onClose();
      return;
    }
    const ok = await perform("oauth-setup", async () => {
      if (addToPool) await relayCommands.setPoolMembership([accountId], [], true);
      if (assignStoredProxy) await relayCommands.assignAutomaticProxies([accountId]);
    }, "feedback.saved", { backgroundRefresh: true });
    if (ok) onClose();
  };
  return <Dialog
    title={t("accounts.accountAdded")}
    onClose={requestClose}
    footer={<>
      <Button variant="secondary" disabled={closeLocked} onClick={requestClose}>{t("accounts.configureLater")}</Button>
      <Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}>{t("common.done")}</Button>
    </>}
  >
    <div className="relay-form oauth-account-setup">
      <div className="oauth-account-added">
        <Check aria-hidden />
        <div>
          <strong>{account?.identityHint ?? t("accounts.accountReady")}</strong>
          <p>{t("accounts.accountAddedHint")}</p>
        </div>
      </div>
      <div className="post-import-options">
        <label>
          <input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} />
          <span><strong>{t("accounts.addAccountToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span>
        </label>
        {hasAccountProxy ? null : <label>
          <input type="checkbox" checked={assignProxy} disabled={!pool || pool.total === 0} onChange={(event) => setAssignProxy(event.target.checked)} />
          <span>
            <strong>{t("proxies.assignStoredAfterAdd")}</strong>
            <small>{pool ? t(pool.total ? "proxies.storedAvailable" : "proxies.noStored", { count: pool.total }) : t("common.loading")}</small>
          </span>
        </label>}
      </div>
    </div>
  </Dialog>;
}

export function SignInProxyDialog({
  entries,
  loading,
  failed,
  initialProxyId,
  onClose,
  onConfirm,
}: {
  entries: ProxyPoolEntry[];
  loading: boolean;
  failed: boolean;
  initialProxyId: string | null;
  onClose: () => void;
  onConfirm: (proxyId: string) => void;
}) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  useEffect(() => {
    const initial = entries.find((entry) => entry.id === initialProxyId && isHttpProxyEndpoint(entry.endpoint));
    setSelectedId(initial?.id ?? null);
  }, [entries, initialProxyId]);
  const selected = entries.find((entry) => entry.id === selectedId && isHttpProxyEndpoint(entry.endpoint));
  return (
    <Dialog
      layer="top"
      title={t("accounts.signInWithProxy")}
      onClose={onClose}
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={!selected} onClick={() => { if (selected) onConfirm(selected.id); }}>{t("accounts.signIn")}</Button>
        </>
      }
    >
      <div className="relay-form proxy-route-form">
        <p className="form-note">{t("accounts.signInProxyDialogHint")}</p>
        {loading ? <div className="center-loading"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div> : null}
        {!loading && failed ? <p role="alert" className="form-note error-text">{t("accounts.signInProxyUnavailable")}</p> : null}
        {!loading && !failed && entries.length === 0 ? <p className="form-note">{t("accounts.signInProxyEmpty")}</p> : null}
        {!loading && !failed && entries.length > 0 ? (
          <div className="proxy-route-options sign-in-proxy-options" role="radiogroup" aria-label={t("accounts.signInWithProxy")}>
            {entries.map((entry) => {
              const http = isHttpProxyEndpoint(entry.endpoint);
              const place = [entry.countryCode, entry.region].filter(Boolean).join(" · ");
              const selectedEntry = entry.id === selected?.id;
              return (
                <button key={entry.id} type="button" role="radio" aria-checked={selectedEntry} disabled={!http} className={selectedEntry ? "selected" : ""} onClick={() => setSelectedId(entry.id)}>
                  <Network aria-hidden />
                  <span>
                    <strong>{entry.endpoint}</strong>
                    <small>{http ? place : t("accounts.signInProxyHttps")}</small>
                  </span>
                  {selectedEntry ? <Check className="proxy-route-check" aria-hidden /> : null}
                </button>
              );
            })}
          </div>
        ) : null}
      </div>
    </Dialog>
  );
}

function formatCountdown(seconds: number) {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}
