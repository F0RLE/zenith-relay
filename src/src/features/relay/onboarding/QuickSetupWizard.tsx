import { lazy, Suspense, useEffect, useState } from "react";
import { ArrowLeft, Check, Cloud, ExternalLink, Languages, Laptop, Loader2, LogIn, Server, SkipForward, Upload, UserRoundCheck, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { ImportSession, RelayMode } from "../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue, type ApiProviderValue } from "../components/ApiProviderForm";
import { sourceSupportsNativeResponses } from "../sourceProtocolBindings";
import { Button, IconButton, OptionMenu, SecretField } from "../components/Ui";
import { useOAuthSignIn } from "../hooks/useOAuthSignIn";
import { useRelayState } from "../state/RelayStateProvider";

const ImportDialog = lazy(async () => ({ default: (await import("../pages/connections/ConnectionsPage")).ImportDialog }));

export function QuickSetupWizard() {
  const { t } = useTranslation();
  const { mode: appMode, runtime, finishOnboarding, perform, activateCodexProfile, busy } = useRelayState();
  const [intro, setIntro] = useState(true);
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<RelayMode>(appMode);
  const [client, setClient] = useState("later");
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [serverUrl, setServerUrl] = useState("");
  const [serverToken, setServerToken] = useState("");
  const [allowInsecureRemote, setAllowInsecureRemote] = useState(false);
  const [connectionReady, setConnectionReady] = useState(false);
  const [apiSourceId, setApiSourceId] = useState<string | null>(null);
  const [showImport, setShowImport] = useState(false);
  const [importSession, setImportSession] = useState<ImportSession | null>(null);
  const [currentProfileAvailable, setCurrentProfileAvailable] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);

  useEffect(() => {
    if (step !== 2) return;
    if (appMode !== mode) return;
    if (mode === "remote" && runtime?.runtimeTarget.connected) setConnectionReady(true);
    if (mode === "zenith" && runtime?.sources.length) setConnectionReady(true);
  }, [appMode, mode, runtime, step]);

  useEffect(() => {
    if (step !== 2 || mode !== "local") {
      setCurrentProfileAvailable(false);
      return;
    }
    let disposed = false;
    setCurrentProfileAvailable(false);
    void relayCommands.currentChatgptProfileAvailable()
      .then((available) => { if (!disposed) setCurrentProfileAvailable(available); })
      .catch(() => undefined);
    return () => { disposed = true; };
  }, [mode, step]);

  const selectMode = (value: RelayMode) => {
    setMode(value);
    setConnectionReady(false);
    setApiSourceId(null);
  };

  const openFileImport = () => {
    setImportSession(null);
    setShowImport(true);
  };

  const openCurrentProfileImport = async () => {
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("onboarding-current-profile", async () => {
      result.current = await relayCommands.previewCurrentCodexImport();
    });
    if (!ok || !result.current) {
      if (result.current) await relayCommands.cancelImport(result.current.sessionId).catch(() => undefined);
      return;
    }
    setImportSession(result.current);
    setShowImport(true);
  };

  const closeImport = () => {
    setShowImport(false);
    setImportSession(null);
  };

  if (intro) {
    return <main className="setup-shell setup-shell-intro">
      <div className="setup-language-floating"><LanguageSelect /></div>
      <section className="product-intro">
        <div className="intro-mark"><img src="/icons/zenith-sword.png" alt="" /></div>
        <div className="intro-copy"><h1>Zenith Relay</h1><p>{t("onboarding.intro")}</p></div>
        <div className="intro-actions"><Button variant="primary" onClick={() => setIntro(false)}>{t("onboarding.start")}</Button><Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={() => finishOnboarding(mode)}>{t("onboarding.skip")}</Button></div>
      </section>
    </main>;
  }

  const insecureRemote = serverUrl.trim().toLowerCase().startsWith("http://");
  const remoteReady = connectionReady || Boolean(serverUrl && serverToken && (!insecureRemote || allowInsecureRemote));
  const canContinue = step === 2
    ? mode === "local" ? !oauthPending : mode === "remote" ? remoteReady : connectionReady || apiProviderReady(provider)
    : true;

  const prepareLocalRuntime = async () => {
    const snapshot = await relayCommands.localState();
    if (!snapshot.gateway.running) await relayCommands.startGateway();
  };

  const next = async () => {
    if (step === 2 && mode === "remote" && !connectionReady) {
      const ok = await perform("onboarding-remote", () => relayCommands.connectRemote({ baseUrl: serverUrl, managementToken: serverToken, allowInsecureHttp: insecureRemote && allowInsecureRemote, confirmIdentityChange: false }), "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "zenith" && !connectionReady) {
      if (!apiProviderReady(provider)) return;
      const ok = await perform("onboarding-api", async () => {
        const created = await relayCommands.createSource(apiProviderSourceInput(provider)) as { id: string };
        setApiSourceId(created.id);
      }, "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "local") {
      const ok = await perform("onboarding-local", prepareLocalRuntime);
      if (!ok) return;
    }
    if (step === 3 && client === "codex" && mode === "local") {
      const ok = await activateCodexProfile("onboarding-client", () => relayCommands.attachCodexGateway());
      if (!ok) return;
    }
    if (step === 3 && client === "codex" && mode === "zenith") {
      const sourceId = apiSourceId ?? runtime?.sources.find(
        (source) =>
          source.enabled
          && source.secretAvailable
          && sourceSupportsNativeResponses(source),
      )?.id;
      if (!sourceId) return;
      const ok = await activateCodexProfile("onboarding-client", () => relayCommands.launchCodexSource(sourceId));
      if (!ok) return;
      localStorage.setItem("relay.directSourceId", sourceId);
    }
    if (step === 4) finishOnboarding(mode);
    else setStep((value) => value + 1);
  };

  return <main className="setup-shell">
    <div className="setup-language-floating"><LanguageSelect /></div>
    <ol className="setup-progress" aria-label={t("onboarding.progress")}>{[1, 2, 3, 4].map((value) => <li key={value} className={value <= step ? "active" : ""}><span>{value < step ? <Check aria-hidden /> : value}</span>{t(`onboarding.steps.${value}`)}</li>)}</ol>
    <section className="setup-body">
      {step === 1 ? <div className="setup-step setup-mode-step"><div className="setup-heading"><h1>{t("onboarding.modeQuestion")}</h1><p>{t("onboarding.modeHint")}</p></div><div className="mode-options">{(["local", "zenith", "remote"] as RelayMode[]).map((value) => { const Icon = value === "local" ? Laptop : value === "remote" ? Server : Cloud; return <button key={value} type="button" className={mode === value ? "selected" : ""} onClick={() => selectMode(value)}><Icon aria-hidden /><span><strong>{t(`modes.${value}`)}</strong><small>{t(`onboarding.modeDescriptions.${value}`)}</small></span><i>{mode === value ? <Check aria-hidden /> : null}</i></button>; })}</div></div> : null}
      {step === 2 ? <div className="setup-step"><ConnectionStep mode={mode} provider={provider} onProviderChange={(value) => { setProvider(value); setConnectionReady(false); }} serverUrl={serverUrl} setServerUrl={(value) => { setServerUrl(value); setConnectionReady(false); }} serverToken={serverToken} setServerToken={(value) => { setServerToken(value); setConnectionReady(false); }} currentProfileAvailable={currentProfileAvailable} onConnected={() => setConnectionReady(true)} onOAuthPendingChange={setOauthPending} onImport={openFileImport} onImportCurrent={() => void openCurrentProfileImport()} />{mode === "remote" && insecureRemote ? <label className="check-line"><input type="checkbox" checked={allowInsecureRemote} onChange={(event) => setAllowInsecureRemote(event.target.checked)} /><span>{t("onboarding.allowInsecureRemote")}</span></label> : null}</div> : null}
      {step === 3 ? <div className="setup-step"><div className="setup-heading"><h1>{t("onboarding.clientQuestion")}</h1><p>{t("onboarding.clientHint")}</p></div><div className="client-options">{["codex", "other", "later"].map((value) => <button type="button" key={value} className={client === value ? "selected" : ""} onClick={() => setClient(value)}><span>{t(`clients.${value}`)}</span><i>{client === value ? <Check aria-hidden /> : null}</i></button>)}</div></div> : null}
      {step === 4 ? <div className="setup-ready"><div className="setup-ready-mark"><Check aria-hidden /></div><h1>{t("onboarding.readyTitle")}</h1><p>{t("onboarding.readyHint", { mode: t(`modes.${mode}`), client: t(`clients.${client}`) })}</p></div> : null}
    </section>
    <footer className="setup-footer"><div><Button variant="ghost" icon={<ArrowLeft aria-hidden />} disabled={step === 1} onClick={() => setStep((value) => Math.max(1, value - 1))}>{t("common.back")}</Button>{step < 4 ? <Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={() => finishOnboarding(mode)}>{t("onboarding.skipStep")}</Button> : null}</div><Button variant="primary" busy={busy?.startsWith("onboarding") ?? false} disabled={!canContinue} onClick={next}>{step === 4 ? t("onboarding.openApp") : t("common.continue")}</Button></footer>
    {showImport ? <Suspense fallback={null}><ImportDialog initialSession={importSession ?? undefined} modeOverride="local" defaultAddToPool onImported={() => setConnectionReady(true)} onClose={closeImport} /></Suspense> : null}
  </main>;
}

function ConnectionStep({ mode, provider, onProviderChange, serverUrl, setServerUrl, serverToken, setServerToken, currentProfileAvailable, onConnected, onOAuthPendingChange, onImport, onImportCurrent }: { mode: RelayMode; provider: ApiProviderValue; onProviderChange: (value: ApiProviderValue) => void; serverUrl: string; setServerUrl: (value: string) => void; serverToken: string; setServerToken: (value: string) => void; currentProfileAvailable: boolean; onConnected: () => void; onOAuthPendingChange: (pending: boolean) => void; onImport: () => void; onImportCurrent: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const oauth = useOAuthSignIn(async (result) => {
    const added = await perform("oauth-pool-membership", () => relayCommands.setPoolMembership([result.account.id], [], true), "feedback.accountAdded");
    if (added) onConnected();
  });
  useEffect(() => {
    onOAuthPendingChange(Boolean(oauth.flow));
    return () => onOAuthPendingChange(false);
  }, [oauth.flow, onOAuthPendingChange]);

  if (mode === "remote") return <><div className="setup-heading"><h1>{t("onboarding.connectionRemote")}</h1><p>{t("onboarding.remoteHint")}</p></div><div className="setup-fields"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={serverToken} onChange={setServerToken} /></div></>;
  if (mode === "zenith") {
    const providerName = provider.kind ? provider.name || t("apiProviders.custom") : null;
    const providerHint = provider.kind === "custom" ? t("apiProviders.configureCustomHint") : t("apiProviders.configureHint");
    return <><div className="setup-heading"><h1>{providerName ? t("apiProviders.configure", { provider: providerName }) : t("onboarding.connectionReady")}</h1><p>{providerName ? providerHint : t("apiProviders.hint")}</p></div><ApiProviderForm value={provider} onChange={onProviderChange} variant="onboarding" /></>;
  }

  const flow = oauth.flow;
  const flowFailed = flow && (flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed");
  const importingCurrent = busy === "onboarding-current-profile";
  return <>
    <div className={`setup-heading${flow ? " compact" : ""}`}>
      <h1>{t("onboarding.connectionLocal")}</h1>
      {!flow ? <p>{t(currentProfileAvailable ? "onboarding.oauthHint" : "onboarding.oauthHintNoProfile")}</p> : null}
    </div>
    {flow ? <section className="setup-oauth-pending" aria-live="polite">
      <div className="setup-oauth-pending-mark"><Loader2 className="spin" aria-hidden /></div>
      <div className="setup-oauth-pending-copy">
        <strong>{t(flow.status === "callback_received" || busy === "oauth-complete" ? "accounts.completingSignIn" : "onboarding.signInWaiting")}</strong>
      </div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="setup-oauth-pending-actions">
        <a className="setup-oauth-reopen" href={flow.authorizationUrl} target="_blank" rel="noreferrer"><ExternalLink aria-hidden /><span>{t("accounts.openSignIn")}</span></a>
        <IconButton label={t("common.cancel")} icon={<X aria-hidden />} disabled={busy === "oauth-cancel"} onClick={() => void oauth.cancel()} />
      </div>
    </section> : <div className={`setup-connect-options${currentProfileAvailable ? " has-current-profile" : ""}`}>
      {currentProfileAvailable ? <button type="button" disabled={importingCurrent} onClick={onImportCurrent}><UserRoundCheck aria-hidden /><span><strong>{t("onboarding.importCurrentProfile")}</strong><small>{t("onboarding.importCurrentProfileDescription")}</small></span></button> : null}
      <button type="button" disabled={busy === "oauth-start" || importingCurrent} onClick={() => void oauth.start()}><LogIn aria-hidden /><span><strong>{t("accounts.signIn")}</strong><small>{t("onboarding.signInDescription")}</small></span></button>
      <button type="button" disabled={importingCurrent} onClick={onImport}><Upload aria-hidden /><span><strong>{t("accounts.import")}</strong><small>{t("onboarding.importDescription")}</small></span></button>
    </div>}
  </>;
}

function LanguageSelect() {
  const { i18n, t } = useTranslation();
  return <OptionMenu
    className="setup-language-menu"
    icon={<Languages aria-hidden />}
    label={t("settings.language")}
    value={i18n.language.startsWith("ru") ? "ru" : "en"}
    onChange={(value) => void i18n.changeLanguage(value)}
    options={[{ value: "ru", label: "Русский", shortLabel: "RU" }, { value: "en", label: "English", shortLabel: "EN" }]}
  />;
}
