import { lazy, Suspense, useEffect, useState } from "react";
import { ArrowLeft, ArrowRight, Check, Clock3, Cloud, ExternalLink, Laptop, Loader2, LogIn, MessageSquare, Server, SkipForward, Terminal, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import relayLogoUrl from "../../../../../src-tauri/icons/zenith-relay.svg?url";
import { setI18nLanguage } from "../../../i18n";
import { relayCommands } from "../api/commands";
import type { OAuthClientKind, RelayMode } from "../api/types";
import { Button, OptionMenu, SecretField } from "../components/Ui";
import { useOAuthSignIn } from "../hooks/useOAuthSignIn";
import { canAccountJoinPool } from "../poolMembership";
import { useRelayState } from "../state/RelayStateProvider";
import { captureOperationResult } from "../state/relayOperationModel";

const ImportDialog = lazy(async () => ({ default: (await import("../pages/connections/ImportDialog")).ImportDialog }));
const SourceDialog = lazy(async () => ({ default: (await import("../pages/connections/SourceDialog")).SourceDialog }));

export function QuickSetupWizard() {
  const { t } = useTranslation();
  const { mode: appMode, runtime, finishOnboarding, perform, activateCodexProfile, busy } = useRelayState();
  const [intro, setIntro] = useState(true);
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<RelayMode>(appMode === "zenith" ? "local" : appMode);
  const [client, setClient] = useState("later");
  const [serverUrl, setServerUrl] = useState("");
  const [serverToken, setServerToken] = useState("");
  const [allowInsecureRemote, setAllowInsecureRemote] = useState(false);
  const [connectionReady, setConnectionReady] = useState(false);
  const [showImport, setShowImport] = useState(false);
  const [showSource, setShowSource] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);

  useEffect(() => {
    if (step !== 2) return;
    if (appMode !== mode) return;
    if (mode === "remote" && runtime?.runtimeTarget.connected) setConnectionReady(true);
  }, [appMode, mode, runtime, step]);

  const selectMode = (selectedMode: RelayMode) => {
    setMode(selectedMode);
    setConnectionReady(false);
  };
  const finishLater = () => finishOnboarding(intro ? appMode : mode);

  const prepareLocalRuntime = async () => {
    const snapshot = await relayCommands.localState();
    if (!snapshot.gateway.running) await relayCommands.startGateway();
  };

  const openFileImport = () => {
    setShowImport(true);
  };

  const closeImport = () => {
    setShowImport(false);
  };

  if (intro) return <SetupIntro onStart={() => setIntro(false)} onSkip={finishLater} />;

  const insecureRemote = serverUrl.trim().toLowerCase().startsWith("http://");
  const remoteReady = connectionReady || Boolean(serverUrl && serverToken && (!insecureRemote || allowInsecureRemote));
  const canContinue = step === 2
    ? mode === "local" ? !oauthPending : remoteReady
    : true;

  const continueSetup = async () => {
    if (step === 2 && mode === "remote" && !connectionReady) {
      const ok = await perform("onboarding-remote", () => relayCommands.connectRemote({
        baseUrl: serverUrl,
        managementToken: serverToken,
        allowInsecureHttp: insecureRemote && allowInsecureRemote,
        confirmIdentityChange: false,
      }), "feedback.connected", { backgroundRefresh: true });
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "local") {
      const ok = await perform("onboarding-local", prepareLocalRuntime, undefined, { backgroundRefresh: true });
      if (!ok) return;
    }
    if (step === 3 && client === "codex" && mode === "local") {
      // Selecting ChatGPT configures the existing profile only. Never launch
      // an application from the wizard; the user can open it later from the
      // normal Gateway/Overview controls.
      const ok = await activateCodexProfile("onboarding-client", () => relayCommands.attachCodexGateway());
      if (!ok) return;
    }
    if (step === 3 && client === "opencode" && mode === "local") {
      // OpenCode is the first supported "other" client. Its configuration is
      // switched to the local pool, with an automatic one-shot recovery copy
      // made by the native command before the first write. Do not launch it.
      const ok = await perform("onboarding-opencode", relayCommands.connectOpenCode, "feedback.saved", { backgroundRefresh: true });
      if (!ok) return;
    }
    if (step === 4) finishOnboarding(mode);
    else setStep((currentStep) => currentStep + 1);
  };

  return <main className="setup-shell">
    <SetupHeader />
    <div className="setup-workspace">
    <SetupProgress step={step} />
    <div className="setup-content">
    <section className="setup-body">
      {step === 1 ? <ModeStep mode={mode} onSelect={selectMode} /> : null}
      {step === 2 ? <div className="setup-step">
        <ConnectionStep
          mode={mode}
          connectionReady={connectionReady}
          serverUrl={serverUrl}
          setServerUrl={(serverUrlValue) => { setServerUrl(serverUrlValue); setConnectionReady(false); }}
          serverToken={serverToken}
          setServerToken={(serverTokenValue) => { setServerToken(serverTokenValue); setConnectionReady(false); }}
          onConnected={() => setConnectionReady(true)}
          onOAuthPendingChange={setOauthPending}
          onImport={openFileImport}
          onAddSource={() => setShowSource(true)}
        />
        {mode === "remote" && insecureRemote ? <label className="check-line">
          <input type="checkbox" checked={allowInsecureRemote} onChange={(event) => setAllowInsecureRemote(event.target.checked)} />
          <span>{t("onboarding.allowInsecureRemote")}</span>
        </label> : null}
      </div> : null}
      {step === 3 ? <ClientStep client={client} onSelect={setClient} /> : null}
      {step === 4 ? <ReadyStep mode={mode} client={client} /> : null}
    </section>
    <footer className="setup-footer">
      <div>
        {step > 1 ? <Button variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={() => setStep((currentStep) => Math.max(1, currentStep - 1))}>{t("common.back")}</Button> : null}
        {step < 3 ? <Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={finishLater}>{t("onboarding.skipStep")}</Button> : null}
      </div>
      <Button variant="primary" busy={busy?.startsWith("onboarding") ?? false} disabled={!canContinue} onClick={continueSetup}>{step === 4 ? t("onboarding.openApp") : t("common.continue")}</Button>
    </footer>
    </div>
    </div>
    {showImport ? (
      <Suspense fallback={null}>
        <ImportDialog
          modeOverride="local"
          defaultAddToPool
          onImported={(addedToPool) => { if (addedToPool) setConnectionReady(true); }}
          onClose={closeImport}
        />
      </Suspense>
    ) : null}
    {showSource ? <Suspense fallback={null}><SourceDialog source={null} modeOverride="local" addToPool onCreated={() => setConnectionReady(true)} onClose={() => setShowSource(false)} /></Suspense> : null}
  </main>;
}

type ConnectionStepProps = {
  mode: RelayMode;
  connectionReady: boolean;
  serverUrl: string;
  setServerUrl: (serverUrlValue: string) => void;
  serverToken: string;
  setServerToken: (serverTokenValue: string) => void;
  onConnected: () => void;
  onOAuthPendingChange: (pending: boolean) => void;
  onImport: () => void;
  onAddSource: () => void;
};

function ConnectionStep({
  mode,
  connectionReady,
  serverUrl,
  setServerUrl,
  serverToken,
  setServerToken,
  onConnected,
  onOAuthPendingChange,
  onImport,
  onAddSource,
}: ConnectionStepProps) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [clientKind, setClientKind] = useState<OAuthClientKind>("codex");
  const oauth = useOAuthSignIn(async (oauthResult) => {
    const added = await captureOperationResult(
      (work) => perform("oauth-pool-membership", work, undefined, { backgroundRefresh: true }),
      async () => {
        const snapshot = await relayCommands.localState();
        const account = snapshot.accounts.find((candidate) => candidate.id === oauthResult.account.id);
        if (!account || !canAccountJoinPool(account)) return false;
        await relayCommands.setPoolMembership([oauthResult.account.id], [], true);
        return true;
      },
    );
    if (added.ok && added.value) onConnected();
  });
  useEffect(() => {
    onOAuthPendingChange(Boolean(oauth.flow));
    return () => onOAuthPendingChange(false);
  }, [oauth.flow, onOAuthPendingChange]);

  if (mode === "remote") return <>
    <div className="setup-heading"><h1>{t("onboarding.connectionRemote")}</h1><p>{t("onboarding.remoteHint")}</p></div>
    <div className="setup-fields">
      <label className="relay-field">
        <span>{t("remote.address")}</span>
        <input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://relay.example.com" />
      </label>
      <SecretField label={t("remote.token")} value={serverToken} onChange={setServerToken} />
    </div>
  </>;
  const flow = oauth.flow;
  const flowFailed = flow && (flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed");
  return <>
    <div className={`setup-heading${flow ? " compact" : ""}`}>
      <h1>{t("onboarding.connectionLocal")}</h1>
      {!flow ? <p>{t("onboarding.poolHint")}</p> : null}
    </div>
    {flow ? <section className="setup-oauth-pending" aria-live="polite">
      <div className="setup-oauth-pending-mark"><Loader2 className="spin" aria-hidden /></div>
      <div className="setup-oauth-pending-copy">
        <strong>{t(flow.status === "callback_received" || busy === "oauth-complete" ? "accounts.completingSignIn" : "onboarding.signInWaiting")}</strong>
        {flow.status === "pending" ? <small>{t("accounts.waitingForSignInHint")}</small> : null}
      </div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="setup-oauth-pending-actions">
        <button
          type="button"
          className="setup-oauth-reopen"
          disabled={flow.status !== "pending" || busy === "oauth-reopen"}
          onClick={() => void perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId), undefined, { backgroundRefresh: true })}
        >
          <ExternalLink aria-hidden />
          <span>{t("accounts.openSignIn")}</span>
        </button>
        <Button variant="ghost" disabled={busy === "oauth-cancel"} onClick={() => void oauth.cancel()}>{t("common.cancel")}</Button>
      </div>
    </section> : <>
      {connectionReady ? <p className="setup-pool-ready" role="status"><Check aria-hidden />{t("onboarding.poolReady")}</p> : null}
      <div className="setup-sign-in-method">
        <OptionMenu
          label={t("accounts.oauthClient")}
          value={clientKind}
          onChange={(kind) => setClientKind(kind as OAuthClientKind)}
          options={[
            { value: "codex", label: t("accounts.oauthChatGpt") },
            { value: "excel_bps", label: t("accounts.oauthExcel") },
          ]}
        />
      </div>
      <div className="setup-connect-options">
        <div className="setup-connect-group"><strong>{t("connections.accounts")}</strong><div className="setup-connect-cards">
          <button type="button" disabled={busy === "oauth-start"} onClick={() => void oauth.start(true, undefined, undefined, clientKind)}>
            <LogIn aria-hidden />
            <span>
              <strong>{t("accounts.signIn")}</strong>
              <small>{t("onboarding.signInDescription")}</small>
            </span>
          </button>
          <button type="button" onClick={onImport}>
            <Upload aria-hidden />
            <span>
              <strong>{t("accounts.import")}</strong>
              <small>{t("onboarding.importDescription")}</small>
            </span>
          </button>
        </div></div>
        <div className="setup-connect-group"><strong>{t("connections.sources")}</strong><div className="setup-connect-cards">
          <button type="button" onClick={onAddSource}><Cloud aria-hidden /><span><strong>{t("onboarding.addApiSource")}</strong><small>{t("onboarding.addApiSourceHint")}</small></span></button>
        </div></div>
      </div>
    </>}
  </>;
}

function SetupHeader() {
  const { t } = useTranslation();
  return <header className="setup-header"><strong>{t("onboarding.setupTitle")}</strong><LanguageSelect /></header>;
}

function SetupIntro({ onStart, onSkip }: { onStart: () => void; onSkip: () => void }) {
  const { t } = useTranslation();
  const sources = [
    { id: "accounts", label: t("onboarding.accounts") },
    { id: "api", label: "API" },
  ];
  const harnesses = [
    { id: "chatgpt", label: "ChatGPT", icon: "/icons/chatgpt.svg" },
    { id: "opencode", label: "OpenCode", icon: "/icons/opencode.svg" },
  ];

  return (
    <main className="setup-shell setup-shell-intro">
      <header className="setup-header setup-header-intro"><LanguageSelect /></header>
      <section className="product-intro">
        <div className="intro-hero">
          <div className="intro-copy">
            <div className="intro-copy-heading">
              <h1>Zenith Relay</h1>
              <p>{t("onboarding.intro")}</p>
            </div>
            <div className="intro-actions">
              <Button variant="primary" onClick={onStart}>{t("onboarding.start")}</Button>
              <Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={onSkip}>{t("onboarding.skip")}</Button>
            </div>
          </div>
          <div className="intro-visual">
            <div className="intro-flow" role="group" aria-label={t("onboarding.flowLabel")}>
              <div className="intro-flow-column intro-flow-sources">
                {sources.map((source) => <div className="intro-flow-node" key={source.id}><span>{source.label}</span></div>)}
              </div>
              <ArrowRight className="intro-flow-arrow" aria-hidden />
              <img className="intro-flow-logo" src={relayLogoUrl} alt="" />
              <ArrowRight className="intro-flow-arrow" aria-hidden />
              <div className="intro-flow-column intro-flow-harnesses">
                {harnesses.map((harness) => <div className="intro-flow-node" key={harness.id}><img src={harness.icon} alt="" /><span>{harness.label}</span></div>)}
              </div>
            </div>
          </div>
        </div>
      </section>
    </main>
  );
}

function SetupProgress({ step }: { step: number }) {
  const { t } = useTranslation();
  return (
    <ol className="setup-progress" aria-label={t("onboarding.progress")}>
      {[1, 2, 3, 4].map((stepNumber) => (
        <li
          key={stepNumber}
          className={stepNumber < step ? "complete" : stepNumber === step ? "active" : ""}
          aria-current={stepNumber === step ? "step" : undefined}
        >
          <span>{stepNumber < step ? <Check aria-hidden /> : stepNumber}</span>
          <div>
            <strong>{t(`onboarding.steps.${stepNumber}`)}</strong>
            <small>{t(`onboarding.stepHints.${stepNumber}`)}</small>
          </div>
        </li>
      ))}
    </ol>
  );
}

function ModeStep({ mode, onSelect }: { mode: RelayMode; onSelect: (mode: RelayMode) => void }) {
  const { t } = useTranslation();
  return (
    <div className="setup-step setup-mode-step">
      <div className="setup-heading"><h1>{t("onboarding.modeQuestion")}</h1></div>
      <div className="mode-options" role="group" aria-label={t("onboarding.steps.1")}>
        {(["local", "remote"] as RelayMode[]).map((relayMode) => {
          const Icon = relayMode === "local" ? Laptop : Server;
          return (
            <button key={relayMode} type="button" aria-pressed={mode === relayMode} className={mode === relayMode ? "selected" : ""} onClick={() => onSelect(relayMode)}>
              <Icon aria-hidden />
              <span><strong>{t(`modes.${relayMode}`)}</strong><small>{t(`onboarding.modeDescriptions.${relayMode}`)}</small></span>
              <i>{mode === relayMode ? <Check aria-hidden /> : null}</i>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function ClientStep({ client, onSelect }: { client: string; onSelect: (client: string) => void }) {
  const { t } = useTranslation();
  return (
    <div className="setup-step">
      <div className="setup-heading"><h1>{t("onboarding.clientQuestion")}</h1><p>{t("onboarding.clientHint")}</p></div>
      <div className="client-options" role="group" aria-label={t("onboarding.steps.3")}>
        {["codex", "opencode", "later"].map((clientOption) => {
          const Icon = clientOption === "codex" ? MessageSquare : clientOption === "opencode" ? Terminal : Clock3;
          return (
            <button
              type="button"
              key={clientOption}
              aria-label={t(`clients.${clientOption}`)}
              aria-describedby={`setup-client-${clientOption}-hint`}
              aria-pressed={client === clientOption}
              className={client === clientOption ? "selected" : ""}
              onClick={() => onSelect(clientOption)}
            >
              <Icon aria-hidden />
              <span><strong>{t(`clients.${clientOption}`)}</strong><small id={`setup-client-${clientOption}-hint`}>{t(`onboarding.clientDescriptions.${clientOption}`)}</small></span>
              <i>{client === clientOption ? <Check aria-hidden /> : null}</i>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function ReadyStep({ mode, client }: { mode: RelayMode; client: string }) {
  const { t } = useTranslation();
  return (
    <div className="setup-ready">
      <div className="setup-ready-mark"><Check aria-hidden /></div>
      <h1>{t("onboarding.readyTitle")}</h1>
      <dl className="setup-ready-summary">
        <div><dt>{t("onboarding.steps.1")}</dt><dd>{t(`modes.${mode}`)}</dd></div>
        <div><dt>{t("onboarding.steps.3")}</dt><dd>{t(`clients.${client}`)}</dd></div>
      </dl>
    </div>
  );
}

function LanguageSelect() {
  const { i18n, t } = useTranslation();
  return <OptionMenu
    className="setup-language-menu"
    listClassName="setup-language-options"
    label={t("settings.language")}
    value={i18n.language.startsWith("ru") ? "ru" : "en"}
    align="center"
    fitContent
    showSelectionIndicator={false}
    onChange={(languageCode) => void setI18nLanguage(languageCode)}
    options={[{ value: "ru", label: "Русский", shortLabel: "RU" }, { value: "en", label: "English", shortLabel: "EN" }]}
  />;
}
