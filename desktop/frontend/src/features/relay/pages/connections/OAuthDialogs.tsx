import { lazy, Suspense, useEffect, useId, useRef, useState } from "react";
import { ArrowRight, Check, ChevronDown, CircleAlert, CircleHelp, Clock3, Copy, ExternalLink, Loader2, Lock, Network } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { OAuthClientKind, OAuthFlow, ProxyPoolEntry } from "../../api/types";
import { Button, Dialog, IconButton, copyText } from "../../components/Ui";
import { AccountLoginNotes } from "../../components/AccountLoginNotes";
import { secondsUntil, useRelativeTimeClock } from "../../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { readRelayPreference, writeRelayPreference } from "../../state/relayPreferences";
import { useRelayState } from "../../state/RelayStateProvider";
import { isHttpProxyEndpoint, rememberedSignInProxyId } from "./signInProxyPreference";
import { OAuthPage } from "./OAuthPage";

const HelpTopicDialog = lazy(async () => ({ default: (await import("../../help/HelpCenter")).HelpTopicDialog }));
const NOTES_EXPANDED = "relay.signInNotesExpanded";

export function OAuthDialog({ flow, notesAccountId, onCancel, onUseProxy, onUseClient, starting = false }: {
  flow: OAuthFlow;
  notesAccountId?: string | undefined;
  onCancel: () => Promise<void>;
  onUseProxy?: ((proxyId: string) => void) | undefined;
  onUseClient?: ((clientKind: OAuthClientKind) => void) | undefined;
  starting?: boolean;
}) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [reopenAt, setReopenAt] = useState(0);
  const [notesOpen, setNotesOpen] = useState(() => readRelayPreference(NOTES_EXPANDED, "0") === "1");
  const [proxyDialog, setProxyDialog] = useState<{ entries: ProxyPoolEntry[]; failed: boolean } | null>(null);
  const [checkingProxies, setCheckingProxies] = useState(false);
  const proxyRequest = useRef<object | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const [readyPageId, setReadyPageId] = useState<string | null>(null);
  const [pageResume, setPageResume] = useState<{ loginId: string; attempt: number; state: "opened" | "failed" }>();
  const resumedPage = pageResume?.loginId === flow.loginId ? pageResume : undefined;
  const pageReady = readyPageId === flow.loginId;
  const pageAttempt = resumedPage?.attempt ?? 0;
  const pageState = resumedPage?.state ?? flow.pageState ?? "closed";
  const clientChoiceId = useId();
  const actionPending = useRef(false);
  const [linkCopied, showLinkCopied, clearLinkCopied] = useTransientFlag(1_500);
  const now = useRelativeTimeClock([flow.expiresAtMs, reopenAt || null]);
  const secondsRemaining = secondsUntil(flow.expiresAtMs, now);
  const reopenIn = secondsUntil(reopenAt, now);
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const completionLabel = flow.diagnostic ? t(`accounts.oauthStage.${flow.diagnostic.stage}`) : t("accounts.completingSignIn");
  const failureStatus = starting ? null : flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed"
    ? flow.status
    : secondsRemaining === 0 && !callbackReceived ? "expired" : null;
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const closeLocked = starting || busy === "oauth-complete" || busy === "oauth-cancel"
    || busy === "oauth-start" || busy === "oauth-reopen";
  const reauthentication = Boolean(flow.targetAccountId);
  // A route sign-in reads card notes without reauthenticating its issuing client.
  const loginNotesAccountId = flow.targetAccountId ?? notesAccountId;
  const clientKind = flow.clientKind ?? "codex";
  const requestClose = () => {
    if (closeLocked || actionPending.current) return;
    proxyRequest.current = null;
    void onCancel();
  };
  const reopen = async () => {
    if (flowUnavailable || closeLocked || checkingProxies || reopenIn > 0 || actionPending.current) return;
    actionPending.current = true;
    try {
      const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId, flow.embedded), undefined, { backgroundRefresh: true });
      if (flow.embedded && !pageReady && (opened || pageState === "closed")) {
        setPageResume({ loginId: flow.loginId, attempt: pageAttempt + 1, state: opened ? "opened" : "failed" });
      } else if (opened && !flow.embedded) {
        setReopenAt(Date.now() + 3_000);
      }
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
    setCheckingProxies(false);
    clearLinkCopied();
    return () => { proxyRequest.current = null; };
  }, [flow.loginId, clearLinkCopied]);
  useEffect(() => {
    if (closeLocked || flowUnavailable) {
      proxyRequest.current = null;
      setCheckingProxies(false);
      setProxyDialog(null);
    }
  }, [closeLocked, flowUnavailable]);
  const selectProxy = async (repeat: boolean) => {
    if (!onUseProxy || flowUnavailable || closeLocked || actionPending.current || proxyRequest.current) return;
    const request = {};
    proxyRequest.current = request;
    setCheckingProxies(true);
    try {
      const { entries } = await relayCommands.getProxyPool();
      if (proxyRequest.current !== request || Date.now() >= flow.expiresAtMs) return;
      const compatible = entries.filter((entry) => isHttpProxyEndpoint(entry.endpoint));
      const remembered = repeat ? compatible.find((entry) => entry.id === rememberedSignInProxyId()) : undefined;
      const selected = remembered ?? (compatible.length === 1 ? compatible[0] : undefined);
      if (selected) onUseProxy(selected.id);
      else setProxyDialog({ entries, failed: false });
    } catch {
      if (proxyRequest.current === request) setProxyDialog({ entries: [], failed: true });
    } finally {
      if (proxyRequest.current === request) {
        proxyRequest.current = null;
        setCheckingProxies(false);
      }
    }
  };
  if (callbackReceived) return <Dialog
    className="sign-in-dialog oauth-sign-in-dialog oauth-completing"
    title={t("accounts.completingSignIn")}
    onClose={() => undefined}
  >
    <div className="oauth-completing-status" role="status">
      <Loader2 className="spin" aria-hidden />
      <span>{completionLabel}</span>
    </div>
  </Dialog>;
  const openLabel = flow.embedded
    ? t(flow.automaticLogin ? "accounts.signInManually" : pageState === "opened" && pageReady && !starting ? "accounts.focusSignInPage" : "accounts.openSignInPage")
    : reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn");
  const openButton = (
    <Button
      variant="primary"
      icon={flow.embedded ? <ArrowRight aria-hidden /> : <ExternalLink aria-hidden />}
      busy={busy === "oauth-reopen"}
      disabled={flowUnavailable || closeLocked || checkingProxies || reopenIn > 0}
      onClick={() => void reopen()}
    >{openLabel}</Button>
  );
  const controls = (
    <div className="relay-form oauth-waiting" aria-busy={starting}>
      <fieldset className="oauth-client-choice" disabled={reauthentication || !onUseClient || flowUnavailable || closeLocked || checkingProxies}>
        <legend className="sr-only">{t("accounts.oauthClient")}</legend>
        <div className="oauth-client-options">
          {([
            { kind: "codex", label: t("accounts.oauthCodex") },
            { kind: "siwc", label: t("accounts.oauthChatGpt") },
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
        {failureStatus && flow.diagnostic?.reason ? <small role="status">
          {t("accounts.oauthDiagnostic", { stage: t(`accounts.oauthStage.${flow.diagnostic.stage}`), reason: flow.diagnostic.reason, elapsed: flow.diagnostic.elapsedMs })}
        </small> : null}
        <div className="oauth-expiry" role={starting || callbackReceived ? "status" : "timer"}>
          <Clock3 aria-hidden />
          <span>{t(starting ? "accounts.preparingSignIn" : callbackReceived ? "accounts.completingSignIn" : "accounts.oauthRemaining")}</span>
          <strong>{starting || callbackReceived ? "—" : formatCountdown(secondsRemaining)}</strong>
        </div>
      </section>
      <div className="oauth-link-actions">
        {onUseProxy && !reauthentication ? (
          <div className={flowUnavailable || closeLocked ? "oauth-open-with-lock is-disabled" : "oauth-open-with-lock"}>
            {openButton}
            <IconButton
              className="oauth-open-lock"
              label={t("accounts.signInWithProxy")}
              icon={<Lock aria-hidden />}
              busy={checkingProxies}
              disabled={flowUnavailable || closeLocked}
              onClick={() => void selectProxy(false)}
              onContextMenu={(event) => {
                event.preventDefault();
                void selectProxy(true);
              }}
            />
          </div>
        ) : openButton}
        <IconButton
          className="oauth-copy-link"
          label={t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}
          icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />}
          disabled={flowUnavailable || closeLocked}
          onClick={() => void copyLink()}
        />
      </div>
      <section className="oauth-notes-card">
        <button type="button" className="oauth-notes-toggle" aria-expanded={notesOpen} onClick={() => {
          setNotesOpen(!notesOpen);
          writeRelayPreference(NOTES_EXPANDED, notesOpen ? "0" : "1");
        }}>
          <span>{t("accounts.loginDetails")}</span>
          <ChevronDown aria-hidden />
        </button>
        {notesOpen ? <AccountLoginNotes key={flow.loginId} {...(loginNotesAccountId ? { accountId: loginNotesAccountId } : {})} loginId={flow.loginId} /> : null}
      </section>
    </div>
  );
  return <>
    <Dialog
      className={`sign-in-dialog oauth-sign-in-dialog${flow.embedded ? " oauth-with-page" : ""}`}
      title={t("accounts.signIn")}
      headerActions={<IconButton label={t("accounts.signInHelp")} icon={<CircleHelp aria-hidden />} onClick={() => setHelpOpen(true)} />}
      onClose={requestClose}
    >
      {flow.embedded ? (
        <div className="oauth-workspace">
          {controls}
          <OAuthPage
            key={`${flow.loginId}:${pageAttempt}`}
            loginId={flow.loginId}
            pageState={pageState}
            suspended={starting || Boolean(proxyDialog) || helpOpen}
            automatic={flow.automaticLogin === true}
            unavailable={flowUnavailable}
            onReady={setReadyPageId}
          />
        </div>
      ) : controls}
    </Dialog>
    {proxyDialog && onUseProxy ? (
      <SignInProxyDialog
        proxyEntries={proxyDialog.entries}
        failed={proxyDialog.failed}
        initialProxyId={rememberedSignInProxyId()}
        onClose={() => setProxyDialog(null)}
        onConfirm={(proxyId) => {
          setProxyDialog(null);
          if (!flowUnavailable && !closeLocked && !actionPending.current) onUseProxy(proxyId);
        }}
      />
    ) : null}
    {helpOpen ? <Suspense fallback={null}><HelpTopicDialog topic="sign-in" onClose={() => setHelpOpen(false)} /></Suspense> : null}
  </>;
}

function SignInProxyDialog({
  proxyEntries,
  failed,
  initialProxyId,
  onClose,
  onConfirm,
}: {
  proxyEntries: ProxyPoolEntry[];
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
        {failed ? <p role="alert" className="form-note error-text">{t("accounts.signInProxyUnavailable")}</p> : null}
        {!failed && proxyEntries.length === 0 ? <p className="form-note">{t("accounts.signInProxyEmpty")}</p> : null}
        {!failed && proxyEntries.length > 0 ? (
          <div className="proxy-route-options sign-in-proxy-options" role="radiogroup" aria-label={t("accounts.signInWithProxy")}>
            {proxyEntries.map((proxyEntry) => {
              const isHttpProxy = isHttpProxyEndpoint(proxyEntry.endpoint);
              const observed = proxyEntry.lastCheck?.countryCode && !proxyEntry.lastCheck.errorCode
                ? [proxyEntry.lastCheck.countryCode, proxyEntry.lastCheck.region, proxyEntry.lastCheck.city].filter(Boolean).join(" · ") : "";
              const declared = [proxyEntry.countryCode, proxyEntry.region].filter(Boolean).join(" · ");
              const proxyLocation = observed || (declared ? t("proxies.declaredLocation", { location: declared }) : "");
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
