import { lazy, Suspense, useEffect, useId, useRef, useState } from "react";
import { Check, ChevronDown, CircleAlert, CircleHelp, Clock3, Copy, ExternalLink, Loader2, Lock, Network } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { OAuthClientKind, OAuthFlow, ProxyPoolEntry } from "../../api/types";
import { Button, Dialog, IconButton, copyText } from "../../components/Ui";
import { AccountLoginNotes } from "../../components/AccountLoginNotes";
import { secondsUntil, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { useRelayState } from "../../state/RelayStateProvider";
import { isHttpProxyEndpoint, rememberedSignInProxyId } from "./signInProxyPreference";
import { useProxyPool } from "./useProxyPool";
import { usePoolAccountWarning } from "../../hooks/usePoolAccountWarning";
import { captureOperationResult } from "../../state/relayOperationModel";

const HelpTopicDialog = lazy(async () => ({ default: (await import("../../help/HelpCenter")).HelpTopicDialog }));

export function OAuthDialog({ flow, onCancel, onUseProxy, onUseClient, starting = false }: {
  flow: OAuthFlow;
  onCancel: () => Promise<void>;
  onUseProxy?: ((proxyId: string) => void) | undefined;
  onUseClient?: ((clientKind: OAuthClientKind) => void) | undefined;
  starting?: boolean;
}) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [reopenAt, setReopenAt] = useState(0);
  const [notesOpen, setNotesOpen] = useState(false);
  const [proxyOpen, setProxyOpen] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const clientChoiceId = useId();
  const actionPending = useRef(false);
  const [linkCopied, showLinkCopied, clearLinkCopied] = useTransientFlag(1_500);
  const now = useRelativeTimeClock([flow.expiresAtMs, reopenAt || null]);
  const secondsRemaining = secondsUntil(flow.expiresAtMs, now);
  const reopenIn = secondsUntil(reopenAt, now);
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const failureStatus = starting ? null : flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed"
    ? flow.status
    : secondsRemaining === 0 && !callbackReceived ? "expired" : null;
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const closeLocked = starting || busy === "oauth-complete" || busy === "oauth-cancel"
    || busy === "oauth-start" || busy === "oauth-reopen";
  const reauthentication = Boolean(flow.targetAccountId);
  const clientKind = flow.clientKind ?? "codex";
  const proxies = useProxyPool(!reauthentication && Boolean(onUseProxy));
  const requestClose = () => {
    if (closeLocked || actionPending.current) return;
    void onCancel();
  };
  const reopen = async () => {
    if (flowUnavailable || closeLocked || reopenIn > 0 || actionPending.current) return;
    actionPending.current = true;
    try {
      const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId), undefined, { backgroundRefresh: true });
      if (opened) setReopenAt(Date.now() + 3_000);
    } finally {
      actionPending.current = false;
    }
  };
  const copyLink = async () => {
    if (flowUnavailable || closeLocked || actionPending.current) return;
    await copyText(flow.authorizationUrl);
    showLinkCopied();
  };
  useEffect(() => {
    setReopenAt(0);
    clearLinkCopied();
  }, [flow.loginId, clearLinkCopied]);
  useEffect(() => {
    if (closeLocked || flowUnavailable) setProxyOpen(false);
  }, [closeLocked, flowUnavailable]);
  const chooseProxy = () => {
    if (!onUseProxy || flowUnavailable || closeLocked || actionPending.current) return;
    setProxyOpen(true);
  };
  const repeatProxy = () => {
    if (!onUseProxy || flowUnavailable || closeLocked || actionPending.current) return;
    const remembered = rememberedSignInProxyId();
    const proxyEntry = proxies.pool?.entries.find((candidateProxy) => candidateProxy.id === remembered);
    if (proxyEntry && isHttpProxyEndpoint(proxyEntry.endpoint)) {
      onUseProxy(proxyEntry.id);
      return;
    }
    setProxyOpen(true);
  };
  return <>
  <Dialog
    className="sign-in-dialog oauth-sign-in-dialog"
    title={t("accounts.signIn")}
    headerActions={<IconButton label={t("accounts.signInHelp")} icon={<CircleHelp aria-hidden />} onClick={() => setHelpOpen(true)} />}
    onClose={requestClose}
  >
    <div className="relay-form oauth-waiting" aria-busy={starting}>
      <fieldset className="oauth-client-choice" disabled={reauthentication || !onUseClient || flowUnavailable || closeLocked}>
        <legend className="sr-only">{t("accounts.oauthClient")}</legend>
        <div className="oauth-client-options">
          {([
            { kind: "codex", label: t("accounts.oauthChatGpt") },
            { kind: "excel_bps", label: t("accounts.oauthExcel") },
          ] as const).map((client) => <label key={client.kind} className="oauth-client-option">
            <input
              className="sr-only"
              type="radio"
              name={clientChoiceId}
              value={client.kind}
              checked={clientKind === client.kind}
              onChange={() => {
                if (reauthentication || flowUnavailable || closeLocked || actionPending.current || clientKind === client.kind) return;
                onUseClient?.(client.kind);
              }}
            />
            <span>{client.label}</span>
          </label>)}
        </div>
      </fieldset>
      <section className="oauth-progress">
        {failureStatus ? <div className="oauth-waiting-status is-error" role="alert">
          <CircleAlert aria-hidden />
          <strong>{t(`accounts.oauthStatus.${failureStatus}`)}</strong>
        </div> : null}
        <div className="oauth-expiry" role={starting || callbackReceived ? "status" : "timer"}>
          <Clock3 aria-hidden />
          <span>{t(starting ? "accounts.preparingSignIn" : callbackReceived ? "accounts.completingSignIn" : "accounts.oauthRemaining")}</span>
          <strong>{starting || callbackReceived ? "—" : formatCountdown(secondsRemaining)}</strong>
        </div>
      </section>
      <div className="oauth-link-actions">
        {onUseProxy && !reauthentication ? (
          <div className={flowUnavailable || closeLocked ? "oauth-open-with-lock is-disabled" : "oauth-open-with-lock"}>
            <Button
              variant="primary"
              icon={<ExternalLink aria-hidden />}
              busy={busy === "oauth-reopen"}
              disabled={flowUnavailable || closeLocked || reopenIn > 0}
              onClick={() => void reopen()}
            >
              {reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}
            </Button>
            <IconButton
              className="oauth-open-lock"
              label={t("accounts.signInWithProxy")}
              title={t("accounts.signInWithProxyHint")}
              icon={<Lock aria-hidden />}
              disabled={flowUnavailable || closeLocked}
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
            disabled={flowUnavailable || closeLocked || reopenIn > 0}
            onClick={() => void reopen()}
          >
            {reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}
          </Button>
        )}
        {clientKind === "codex" ? <IconButton
          className="oauth-copy-link"
          label={t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}
          icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />}
          disabled={flowUnavailable || closeLocked}
          onClick={() => void copyLink()}
        /> : null}
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
      proxyEntries={proxies.pool?.entries ?? []}
      loading={!proxies.pool && !proxies.failed}
      failed={proxies.failed}
      initialProxyId={rememberedSignInProxyId()}
      onClose={() => setProxyOpen(false)}
      onConfirm={(proxyId) => {
        setProxyOpen(false);
        if (!flowUnavailable && !closeLocked && !actionPending.current) onUseProxy(proxyId);
      }}
    />
  ) : null}
  {helpOpen ? <Suspense fallback={null}><HelpTopicDialog topic="sign-in" onClose={() => setHelpOpen(false)} /></Suspense> : null}
  </>;
}

export function OAuthAccountSetupDialog({ accountId, preserveProxy = false, onClose }: { accountId: string; preserveProxy?: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const confirmPoolAccounts = usePoolAccountWarning();
  const applying = useRef(false);
  const [addToPool, setAddToPool] = useState(false);
  const [assignProxy, setAssignProxy] = useState(false);
  const account = runtime?.accounts.find((candidateAccount) => candidateAccount.id === accountId);
  const hasAccountProxy = preserveProxy || account?.proxyMode === "account";
  const closeLocked = busy === "oauth-setup";
  const requestClose = () => {
    if (closeLocked || applying.current) return;
    onClose();
  };
  const apply = async (bypassWarning = false) => {
    if (applying.current) return;
    applying.current = true;
    const assignStoredProxy = assignProxy && !hasAccountProxy;
    try {
      if (!addToPool && !assignStoredProxy) {
        onClose();
        return;
      }
      const captured = addToPool && !account ? await captureOperationResult(
        (work) => perform("oauth-setup-state", work, undefined, { backgroundRefresh: true }),
        () => relayCommands.localState(),
      ) : null;
      if (captured && !captured.ok) return;
      const freshAccount = account ?? captured?.value?.accounts.find((candidate) => candidate.id === accountId);
      const includeInPool = addToPool && await confirmPoolAccounts(freshAccount ? [freshAccount] : [{}], bypassWarning);
      if (!includeInPool && !assignStoredProxy) {
        onClose();
        return;
      }
      const ok = await perform("oauth-setup", async () => {
        if (includeInPool) await relayCommands.setPoolMembership([accountId], [], true);
        if (assignStoredProxy) await relayCommands.assignAutomaticProxies([accountId]);
      }, "feedback.saved", { backgroundRefresh: true });
      if (ok) onClose();
    } finally {
      applying.current = false;
    }
  };
  return <Dialog
    title={t("accounts.accountAdded")}
    onClose={requestClose}
    footer={<>
      <Button variant="secondary" disabled={closeLocked} onClick={requestClose}>{t("accounts.configureLater")}</Button>
      <Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}
        data-relay-context-action onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          void apply(true);
        }}
      >{t("common.done")}</Button>
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
  proxyEntries,
  loading,
  failed,
  initialProxyId,
  onClose,
  onConfirm,
}: {
  proxyEntries: ProxyPoolEntry[];
  loading: boolean;
  failed: boolean;
  initialProxyId: string | null;
  onClose: () => void;
  onConfirm: (proxyId: string) => void;
}) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  useEffect(() => {
    const initial = proxyEntries.find((candidateProxy) => candidateProxy.id === initialProxyId && isHttpProxyEndpoint(candidateProxy.endpoint));
    setSelectedId(initial?.id ?? null);
  }, [proxyEntries, initialProxyId]);
  const selected = proxyEntries.find((candidateProxy) => candidateProxy.id === selectedId && isHttpProxyEndpoint(candidateProxy.endpoint));
  return (
    <Dialog
      layer="top"
      className="sign-in-dialog sign-in-proxy-dialog"
      title={t("accounts.signInWithProxy")}
      onClose={onClose}
      footer={
        <Button variant="secondary" disabled={!selected} onClick={() => { if (selected) onConfirm(selected.id); }}>{t("accounts.signIn")}</Button>
      }
    >
      <div className="relay-form proxy-route-form">
        {loading ? <div className="center-loading"><Loader2 className="spin" aria-hidden />{t("common.loading")}</div> : null}
        {!loading && failed ? <p role="alert" className="form-note error-text">{t("accounts.signInProxyUnavailable")}</p> : null}
        {!loading && !failed && proxyEntries.length === 0 ? <p className="form-note">{t("accounts.signInProxyEmpty")}</p> : null}
        {!loading && !failed && proxyEntries.length > 0 ? (
          <div className="proxy-route-options sign-in-proxy-options" role="radiogroup" aria-label={t("accounts.signInWithProxy")}>
            {proxyEntries.map((proxyEntry) => {
              const isHttpProxy = isHttpProxyEndpoint(proxyEntry.endpoint);
              const proxyLocation = [proxyEntry.countryCode, proxyEntry.region].filter(Boolean).join(" · ");
              const isSelected = proxyEntry.id === selected?.id;
              return (
                <button key={proxyEntry.id} type="button" role="radio" aria-checked={isSelected} disabled={!isHttpProxy} className={isSelected ? "selected" : ""} onClick={() => setSelectedId(proxyEntry.id)}>
                  <Network aria-hidden />
                  <span>
                    <strong>{proxyEntry.endpoint}</strong>
                    {!isHttpProxy || proxyLocation ? <small>{isHttpProxy ? proxyLocation : t("accounts.signInProxyHttps")}</small> : null}
                  </span>
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
