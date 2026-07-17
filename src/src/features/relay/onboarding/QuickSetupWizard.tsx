import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Check, CircleAlert, Cloud, Languages, Laptop, Loader2, LogIn, Server, SkipForward, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { getSavedKeyModels, getSavedKeyStats, saveKey } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { RelayMode } from "../api/types";
import { ApiProviderForm, apiProviderReady, apiProviderSourceInput, defaultApiProviderValue, type ApiProviderValue } from "../components/ApiProviderForm";
import { Button, OptionMenu, SecretField } from "../components/Ui";
import { useOAuthSignIn } from "../hooks/useOAuthSignIn";
import { ImportDialog } from "../pages/connections/ConnectionsPage";
import { useRelayState } from "../state/RelayStateProvider";

type CheckStatus = "pending" | "running" | "success" | "error";
const checkNames = ["credentials", "endpoint", "models", "capacity"] as const;

export function QuickSetupWizard() {
  const { t } = useTranslation();
  const { mode: appMode, runtime, readyState, finishOnboarding, perform, activateCodexProfile, busy } = useRelayState();
  const [intro, setIntro] = useState(true);
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<RelayMode>(appMode);
  const [client, setClient] = useState("later");
  const [provider, setProvider] = useState(defaultApiProviderValue);
  const [serverUrl, setServerUrl] = useState("");
  const [serverToken, setServerToken] = useState("");
  const [allowInsecureRemote, setAllowInsecureRemote] = useState(false);
  const [connectionReady, setConnectionReady] = useState(false);
  const [checkStages, setCheckStages] = useState<Record<string, CheckStatus>>(() => pendingChecks());
  const [checkError, setCheckError] = useState(false);
  const [localKeyId, setLocalKeyId] = useState<string | null>(null);
  const [showImport, setShowImport] = useState(false);

  useEffect(() => {
    if (step !== 2) return;
    if (appMode !== mode) return;
    if (mode === "local" && runtime?.gateway.candidateCount) setConnectionReady(true);
    if (mode === "remote" && runtime?.runtimeTarget.connected) setConnectionReady(true);
    if (mode === "zenith" && readyState?.providerActive) setConnectionReady(true);
  }, [appMode, mode, readyState?.providerActive, runtime, step]);

  const runCheck = useCallback(async () => {
    setCheckStages(pendingChecks());
    setCheckError(false);
    let active: typeof checkNames[number] = "credentials";
    const mark = (name: string, status: CheckStatus) => setCheckStages((current) => ({ ...current, [name]: status }));
    try {
      mark(active, "running");
      if (mode === "local") {
        let snapshot = await relayCommands.localState();
        if (!snapshot.accounts.length && !snapshot.sources.length) throw new Error("missing local connection");
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        let key = snapshot.keys.find((candidate) => candidate.system && candidate.enabled);
        if (!key) {
          const created = await relayCommands.createKey(t("onboarding.defaultKey"), true);
          key = created.key;
        }
        setLocalKeyId(key.id);
        if (!snapshot.gateway.running) await relayCommands.startGateway();
        snapshot = await relayCommands.localState();
        if (!snapshot.gateway.baseUrl) throw new Error("local endpoint is missing");
        mark(active, "success"); active = "models"; mark(active, "running");
        if (!snapshot.gateway.visibleModelIds.length) throw new Error("no visible models");
        mark(active, "success"); active = "capacity"; mark(active, "running");
        if (!snapshot.gateway.candidateCount) throw new Error("no eligible pool members");
      } else if (mode === "remote") {
        const snapshot = await relayCommands.remoteState();
        if (!snapshot?.runtimeTarget.connected) throw new Error("remote server is not connected");
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        if (!snapshot.gateway.baseUrl) throw new Error("remote endpoint is missing");
        mark(active, "success"); active = "models"; mark(active, "running");
        if (!snapshot.gateway.visibleModelIds.length) throw new Error("no visible models");
        mark(active, "success"); active = "capacity"; mark(active, "running");
        if (!snapshot.gateway.candidateCount) throw new Error("no eligible pool members");
      } else {
        await getSavedKeyStats();
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        mark(active, "success"); active = "models"; mark(active, "running");
        await getSavedKeyModels();
        mark(active, "success"); active = "capacity"; mark(active, "running");
      }
      mark(active, "success");
    } catch {
      mark(active, "error");
      setCheckError(true);
    }
  }, [mode, t]);

  useEffect(() => { if (step === 3) void runCheck(); }, [runCheck, step]);

  const selectMode = (value: RelayMode) => {
    setMode(value);
    setConnectionReady(false);
    setCheckError(false);
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

  const checkComplete = Object.values(checkStages).every((status) => status === "success");
  const insecureRemote = serverUrl.trim().toLowerCase().startsWith("http://");
  const remoteReady = connectionReady || Boolean(serverUrl && serverToken && (!insecureRemote || allowInsecureRemote));
  const canContinue = step === 2
    ? mode === "local" ? connectionReady : mode === "remote" ? remoteReady : connectionReady || apiProviderReady(provider)
    : step === 3 ? checkComplete && !checkError
      : true;

  const next = async () => {
    if (step === 2 && mode === "remote" && !connectionReady) {
      const ok = await perform("onboarding-remote", () => relayCommands.connectRemote({ baseUrl: serverUrl, managementToken: serverToken, allowInsecureHttp: insecureRemote && allowInsecureRemote, confirmIdentityChange: false }), "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "zenith" && !connectionReady) {
      if (!apiProviderReady(provider)) return;
      const ok = await perform("onboarding-api", async () => {
        if (provider.kind === "zenith") {
          await saveKey(provider.apiKey);
          return;
        }
        const created = await relayCommands.createSource(apiProviderSourceInput(provider)) as { id: string };
        await relayCommands.setPoolMembership([], [created.id], true);
      }, "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
      if (provider.kind !== "zenith") {
        setMode("local");
      }
    }
    if (step === 4 && client === "codex" && mode === "local") {
      if (!localKeyId) return;
      const ok = await activateCodexProfile("onboarding-client", () => relayCommands.attachCodexGateway(localKeyId, null));
      if (!ok) return;
    }
    if (step === 5) finishOnboarding(mode);
    else setStep((value) => value + 1);
  };

  return <main className="setup-shell">
    <div className="setup-language-floating"><LanguageSelect /></div>
    <ol className="setup-progress" aria-label={t("onboarding.progress")}>{[1, 2, 3, 4, 5].map((value) => <li key={value} className={value <= step ? "active" : ""}><span>{value < step ? <Check aria-hidden /> : value}</span>{t(`onboarding.steps.${value}`)}</li>)}</ol>
    <section className="setup-body">
      {step === 1 ? <><div className="setup-heading"><h1>{t("onboarding.modeQuestion")}</h1><p>{t("onboarding.modeHint")}</p></div><div className="mode-options">{(["local", "zenith", "remote"] as RelayMode[]).map((value) => { const Icon = value === "local" ? Laptop : value === "remote" ? Server : Cloud; return <button key={value} type="button" className={mode === value ? "selected" : ""} onClick={() => selectMode(value)}><Icon aria-hidden /><span><strong>{t(`modes.${value}`)}</strong><small>{t(`onboarding.modeDescriptions.${value}`)}</small></span><i>{mode === value ? <Check aria-hidden /> : null}</i></button>; })}</div></> : null}
      {step === 2 ? <ConnectionStep mode={mode} provider={provider} onProviderChange={(value) => { setProvider(value); setConnectionReady(false); }} serverUrl={serverUrl} setServerUrl={(value) => { setServerUrl(value); setConnectionReady(false); }} serverToken={serverToken} setServerToken={(value) => { setServerToken(value); setConnectionReady(false); }} connectionReady={connectionReady} onConnected={() => setConnectionReady(true)} onImport={() => setShowImport(true)} /> : null}
      {step === 2 && mode === "remote" && insecureRemote ? <label className="check-line"><input type="checkbox" checked={allowInsecureRemote} onChange={(event) => setAllowInsecureRemote(event.target.checked)} /><span>{t("onboarding.allowInsecureRemote")}</span></label> : null}
      {step === 3 ? <><div className="setup-heading"><h1>{t("onboarding.checkTitle")}</h1><p>{t("onboarding.checkHint")}</p></div><ul className="check-stages">{checkNames.map((value) => { const status = checkStages[value]; return <li key={value}>{status === "running" ? <Loader2 className="spin" aria-hidden /> : status === "error" ? <CircleAlert aria-hidden /> : <Check aria-hidden />}<span>{t(`onboarding.checks.${value}`)}</span><strong>{t(`onboarding.checkStates.${status}`)}</strong></li>; })}</ul>{checkError ? <div className="check-retry"><p role="alert">{t("onboarding.checkFailedHint")}</p><Button variant="secondary" onClick={runCheck}>{t("common.retry")}</Button></div> : null}</> : null}
      {step === 4 ? <><div className="setup-heading"><h1>{t("onboarding.clientQuestion")}</h1><p>{t("onboarding.clientHint")}</p></div><div className="client-options">{["codex", "other", "later"].map((value) => <button type="button" key={value} className={client === value ? "selected" : ""} onClick={() => setClient(value)}><span>{t(`clients.${value}`)}</span><i>{client === value ? <Check aria-hidden /> : null}</i></button>)}</div></> : null}
      {step === 5 ? <div className="setup-ready"><div className="setup-ready-mark"><Check aria-hidden /></div><h1>{t("onboarding.readyTitle")}</h1><p>{t("onboarding.readyHint", { mode: t(`modes.${mode}`), client: t(`clients.${client}`) })}</p></div> : null}
    </section>
    <footer className="setup-footer"><div><Button variant="ghost" icon={<ArrowLeft aria-hidden />} disabled={step === 1} onClick={() => setStep((value) => Math.max(1, value - 1))}>{t("common.back")}</Button>{step < 5 ? <Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={() => finishOnboarding(mode)}>{t("onboarding.skipStep")}</Button> : null}</div><Button variant="primary" busy={busy?.startsWith("onboarding") ?? false} disabled={!canContinue} onClick={next}>{step === 5 ? t("onboarding.openApp") : t("common.continue")}</Button></footer>
    {showImport ? <ImportDialog modeOverride="local" defaultAddToPool onImported={() => setConnectionReady(true)} onClose={() => setShowImport(false)} /> : null}
  </main>;
}

function ConnectionStep({ mode, provider, onProviderChange, serverUrl, setServerUrl, serverToken, setServerToken, connectionReady, onConnected, onImport }: { mode: RelayMode; provider: ApiProviderValue; onProviderChange: (value: ApiProviderValue) => void; serverUrl: string; setServerUrl: (value: string) => void; serverToken: string; setServerToken: (value: string) => void; connectionReady: boolean; onConnected: () => void; onImport: () => void }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const oauth = useOAuthSignIn(async (result) => {
    const added = await perform("oauth-pool-membership", () => relayCommands.setPoolMembership([result.account.id], [], true), "feedback.accountAdded");
    if (added) onConnected();
  });

  if (mode === "remote") return <><div className="setup-heading"><h1>{t("onboarding.connectionRemote")}</h1><p>{t("onboarding.remoteHint")}</p></div><div className="setup-fields"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={serverToken} onChange={setServerToken} /></div>{connectionReady ? <ConnectedLine /> : null}</>;
  if (mode === "zenith") return <><div className="setup-heading"><h1>{t("onboarding.connectionReady")}</h1><p>{t("apiProviders.hint")}</p></div><ApiProviderForm value={provider} onChange={onProviderChange} />{connectionReady ? <ConnectedLine /> : null}</>;

  const flowFailed = oauth.flow && (oauth.flow.status === "callback_rejected" || oauth.flow.status === "expired" || oauth.flow.status === "failed");
  return <><div className="setup-heading"><h1>{t("onboarding.connectionLocal")}</h1><p>{t("onboarding.oauthHint")}</p></div><div className="setup-connect-options"><button type="button" disabled={Boolean(oauth.flow) || busy === "oauth-start"} onClick={() => void oauth.start()}><LogIn aria-hidden /><span><strong>{t("accounts.signIn")}</strong><small>{t("onboarding.signInDescription")}</small></span></button><button type="button" disabled={Boolean(oauth.flow)} onClick={onImport}><Upload aria-hidden /><span><strong>{t("accounts.import")}</strong><small>{t("onboarding.importDescription")}</small></span></button></div>{connectionReady ? <ConnectedLine /> : null}{oauth.flow ? <div className="oauth-progress"><div className="oauth-waiting-status"><Loader2 className="spin" aria-hidden /><div><strong>{t(oauth.flow.status === "callback_received" || busy === "oauth-complete" ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong><p>{t("accounts.waitingForSignInHint")}</p></div></div>{flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${oauth.flow.status}`)}</p> : null}<div className="inline-actions"><a href={oauth.flow.authorizationUrl} target="_blank" rel="noreferrer">{t("accounts.reopenSignIn")}</a><Button variant="ghost" busy={busy === "oauth-cancel"} onClick={() => void oauth.cancel()}>{t("common.cancel")}</Button></div></div> : null}</>;
}

function ConnectedLine() {
  const { t } = useTranslation();
  return <div className="setup-connected"><Check aria-hidden /><span>{t("onboarding.connectionDetected")}</span></div>;
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

function pendingChecks() {
  return Object.fromEntries(checkNames.map((name) => [name, "pending"])) as Record<string, CheckStatus>;
}
