import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Check, CircleAlert, Cloud, Copy, Laptop, Loader2, Server, SkipForward } from "lucide-react";
import { useTranslation } from "react-i18next";
import { getSavedKeyModels, getSavedKeyStats, saveKey } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { RelayMode } from "../api/types";
import { Button, SecretField, copyText } from "../components/Ui";
import { useOAuthSignIn } from "../hooks/useOAuthSignIn";
import { useRelayState } from "../state/RelayStateProvider";

export function QuickSetupWizard() {
  const { t } = useTranslation();
  const { finishOnboarding, perform, activateCodexProfile, busy } = useRelayState();
  const [intro, setIntro] = useState(true);
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<RelayMode>("local");
  const [client, setClient] = useState("later");
  const [apiKey, setApiKey] = useState("");
  const [serverUrl, setServerUrl] = useState("");
  const [serverToken, setServerToken] = useState("");
  const [allowInsecureRemote, setAllowInsecureRemote] = useState(false);
  const [connectionReady, setConnectionReady] = useState(false);
  const [allowKeyCreation, setAllowKeyCreation] = useState(false);
  const [checkStages, setCheckStages] = useState<Record<string, "pending" | "running" | "success" | "error">>({ credentials: "pending", endpoint: "pending", models: "pending", capacity: "pending" });
  const [checkError, setCheckError] = useState(false);
  const [checkedEndpoint, setCheckedEndpoint] = useState("");
  const [localKeyId, setLocalKeyId] = useState<string | null>(null);

  const runCheck = useCallback(async () => {
    const stages = ["credentials", "endpoint", "models", "capacity"];
    const reset = Object.fromEntries(stages.map((stage) => [stage, "pending"])) as Record<string, "pending" | "running" | "success" | "error">;
    setCheckStages(reset); setCheckError(false); let active = "credentials";
    const mark = (stage: string, status: "running" | "success" | "error") => setCheckStages((current) => ({ ...current, [stage]: status }));
    try {
      mark(active, "running");
      if (mode === "local") {
        let snapshot = await relayCommands.localState();
        if (!snapshot.accounts.length && !snapshot.sources.length) throw new Error("missing local connection");
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        let key = snapshot.keys.find((candidate) => candidate.enabled);
        if (!key) {
          if (!allowKeyCreation) throw new Error("local key creation was not confirmed");
          const created = await relayCommands.createKey(t("onboarding.defaultKey"));
          key = created.key;
        }
        setLocalKeyId(key.id);
        if (!snapshot.gateway.running) await relayCommands.startGateway();
        snapshot = await relayCommands.localState();
        setCheckedEndpoint(snapshot.gateway.baseUrl); mark(active, "success"); active = "models"; mark(active, "running");
        if (!snapshot.gateway.visibleModelIds.length) throw new Error("no visible models");
        mark(active, "success"); active = "capacity"; mark(active, "running");
        if (!snapshot.gateway.candidateCount) throw new Error("no eligible pool members");
      } else if (mode === "remote") {
        const snapshot = await relayCommands.remoteState();
        if (!snapshot?.runtimeTarget.connected) throw new Error("remote server is not connected");
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        if (!snapshot.gateway.baseUrl) throw new Error("remote endpoint is missing");
        setCheckedEndpoint(snapshot.gateway.baseUrl); mark(active, "success"); active = "models"; mark(active, "running");
        if (!snapshot.gateway.visibleModelIds.length) throw new Error("no visible models");
        mark(active, "success"); active = "capacity"; mark(active, "running");
        if (!snapshot.gateway.candidateCount) throw new Error("no eligible pool members");
      } else {
        await getSavedKeyStats();
        mark(active, "success"); active = "endpoint"; mark(active, "running");
        setCheckedEndpoint("https://api.zenithmarket.dev/v1"); mark(active, "success"); active = "models"; mark(active, "running");
        await getSavedKeyModels();
        mark(active, "success"); active = "capacity"; mark(active, "running");
      }
      mark(active, "success");
    } catch {
      mark(active, "error"); setCheckError(true);
    }
  }, [allowKeyCreation, mode, t]);

  useEffect(() => { if (step === 3) void runCheck(); }, [runCheck, step]);

  if (intro) {
    return <main className="setup-shell"><header className="setup-top"><strong>Zenith Relay</strong><LanguageSelect /></header><section className="product-intro"><div className="intro-copy"><h1>Zenith Relay</h1><p>{t("onboarding.intro")}</p><div className="flow-line" aria-label={t("onboarding.flowLabel")}><span>{t("onboarding.accounts")}</span><i /><strong>Zenith Relay</strong><i /><span>{t("onboarding.apps")}</span></div><dl><div><dt>{t("modes.local")}</dt><dd>{t("onboarding.factLocal")}</dd></div><div><dt>{t("modes.remote")}</dt><dd>{t("onboarding.factRemote")}</dd></div><div><dt>{t("modes.zenith")}</dt><dd>{t("onboarding.factReady")}</dd></div></dl></div><div className="intro-actions"><Button variant="primary" onClick={() => setIntro(false)}>{t("onboarding.start")}</Button><Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={() => finishOnboarding("local")}>{t("onboarding.skip")}</Button></div></section></main>;
  }

  const checkComplete = Object.values(checkStages).every((status) => status === "success");
  const insecureRemote = serverUrl.trim().toLowerCase().startsWith("http://");
  const canContinue = step === 2 ? (mode === "local" ? connectionReady : mode === "remote" ? Boolean(serverUrl && serverToken && (!insecureRemote || allowInsecureRemote)) : Boolean(apiKey)) : step === 3 ? checkComplete && !checkError : true;
  const next = async () => {
    if (step === 2 && mode === "remote") {
      const ok = await perform("onboarding-remote", () => relayCommands.connectRemote({ baseUrl: serverUrl, managementToken: serverToken, allowInsecureHttp: insecureRemote && allowInsecureRemote, confirmIdentityChange: false }), "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "zenith") {
      const ok = await perform("onboarding-ready", () => saveKey(apiKey), "feedback.saved");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 4 && client === "codex" && mode === "local") {
      if (!localKeyId) return;
      const ok = await activateCodexProfile("onboarding-client", () => relayCommands.attachCodexGateway(localKeyId, null));
      if (!ok) return;
    }
    if (step === 4 && client === "opencode" && mode === "local") {
      if (!localKeyId) return;
      const ok = await perform("onboarding-client", () => relayCommands.attachOpenCodeGateway(localKeyId), "feedback.profileAttached");
      if (!ok) return;
    }
    if (step === 5) finishOnboarding(mode);
    else setStep((value) => value + 1);
  };

  return <main className="setup-shell"><header className="setup-top"><strong>Zenith Relay</strong><LanguageSelect /></header><ol className="setup-progress" aria-label={t("onboarding.progress")}>{[1,2,3,4,5].map((value) => <li key={value} className={value <= step ? "active" : ""}><span>{value < step ? <Check aria-hidden /> : value}</span>{t(`onboarding.steps.${value}`)}</li>)}</ol><section className="setup-body">
    {step === 1 ? <><h1>{t("onboarding.modeQuestion")}</h1><p>{t("onboarding.modeHint")}</p><div className="mode-options">{(["local","remote","zenith"] as RelayMode[]).map((value) => { const Icon = value === "local" ? Laptop : value === "remote" ? Server : Cloud; return <button key={value} type="button" className={mode === value ? "selected" : ""} onClick={() => { setMode(value); setConnectionReady(false); setCheckError(false); setCheckedEndpoint(""); }}><Icon aria-hidden /><span><strong>{t(`modes.${value}`)}</strong><small>{t(`onboarding.modeDescriptions.${value}`)}</small></span>{mode === value ? <Check aria-hidden /> : null}</button>; })}</div></> : null}
    {step === 2 ? <><ConnectionStep mode={mode} apiKey={apiKey} setApiKey={setApiKey} serverUrl={serverUrl} setServerUrl={setServerUrl} serverToken={serverToken} setServerToken={setServerToken} onConnected={() => setConnectionReady(true)} />{mode === "local" && connectionReady ? <label className="check-line"><input type="checkbox" checked={allowKeyCreation} onChange={(event) => setAllowKeyCreation(event.target.checked)} /><span>{t("onboarding.allowKeyCreation")}</span></label> : null}{mode === "remote" && insecureRemote ? <label className="check-line"><input type="checkbox" checked={allowInsecureRemote} onChange={(event) => setAllowInsecureRemote(event.target.checked)} /><span>{t("onboarding.allowInsecureRemote")}</span></label> : null}</> : null}
    {step === 3 ? <><h1>{t("onboarding.checkTitle")}</h1><p>{t("onboarding.checkHint")}</p><ul className="check-stages">{["credentials","endpoint","models","capacity"].map((value) => { const status = checkStages[value]; return <li key={value}>{status === "running" ? <Loader2 className="spin" aria-hidden /> : status === "error" ? <CircleAlert aria-hidden /> : <Check aria-hidden />}<span>{t(`onboarding.checks.${value}`)}</span><strong>{t(`onboarding.checkStates.${status}`)}</strong></li>; })}</ul>{checkError ? <Button variant="secondary" onClick={runCheck}>{t("common.retry")}</Button> : null}</> : null}
    {step === 4 ? <><h1>{t("onboarding.clientQuestion")}</h1><p>{t("onboarding.clientHint")}</p><div className="client-options">{["codex","opencode","other","later"].map((value) => <button type="button" key={value} className={client === value ? "selected" : ""} onClick={() => setClient(value)}>{t(`clients.${value}`)}{client === value ? <Check aria-hidden /> : null}</button>)}</div></> : null}
    {step === 5 ? <div className="setup-ready"><Check aria-hidden /><h1>{t("onboarding.readyTitle")}</h1><p>{t("onboarding.readyHint", { mode: t(`modes.${mode}`), client: t(`clients.${client}`) })}</p><div className="endpoint-line"><code>{checkedEndpoint}</code><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(checkedEndpoint)}>{t("common.copy")}</Button></div></div> : null}
  </section><footer className="setup-footer"><Button variant="ghost" icon={<ArrowLeft aria-hidden />} disabled={step === 1} onClick={() => setStep((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><Button variant="primary" busy={busy?.startsWith("onboarding") ?? false} disabled={!canContinue} onClick={next}>{step === 5 ? t("onboarding.openApp") : t("common.continue")}</Button></footer></main>;
}

function ConnectionStep({ mode, apiKey, setApiKey, serverUrl, setServerUrl, serverToken, setServerToken, onConnected }: { mode: RelayMode; apiKey: string; setApiKey: (value: string) => void; serverUrl: string; setServerUrl: (value: string) => void; serverToken: string; setServerToken: (value: string) => void; onConnected: () => void }) {
  const { t } = useTranslation();
  const { busy } = useRelayState();
  const oauth = useOAuthSignIn(onConnected);
  if (mode === "remote") return <><h1>{t("onboarding.connectionRemote")}</h1><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={serverToken} onChange={setServerToken} /></>;
  if (mode === "zenith") return <><h1>{t("onboarding.connectionReady")}</h1><div className="recommended-line"><strong>Zenith API</strong><span>{t("common.recommended")}</span></div><SecretField label={t("readyApi.key")} value={apiKey} onChange={setApiKey} /></>;
  const flowFailed = oauth.flow && (oauth.flow.status === "callback_rejected" || oauth.flow.status === "expired" || oauth.flow.status === "failed");
  return <><h1>{t("onboarding.connectionLocal")}</h1><p>{t("onboarding.oauthHint")}</p>{oauth.flow ? <div className="oauth-progress"><div className="oauth-waiting-status"><Loader2 className="spin" aria-hidden /><div><strong>{t(oauth.flow.status === "callback_received" || busy === "oauth-complete" ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong><p>{t("accounts.waitingForSignInHint")}</p></div></div>{flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${oauth.flow.status}`)}</p> : null}<a href={oauth.flow.authorizationUrl} target="_blank" rel="noreferrer">{t("accounts.reopenSignIn")}</a><Button variant="ghost" busy={busy === "oauth-cancel"} onClick={() => void oauth.cancel()}>{t("common.cancel")}</Button></div> : <Button variant="primary" busy={busy === "oauth-start"} onClick={() => void oauth.start()}>{t("accounts.signIn")}</Button>}<p className="form-note">{t("onboarding.importLater")}</p></>;
}

function LanguageSelect() {
  const { i18n, t } = useTranslation();
  return <label className="language-select"><span>{t("settings.language")}</span><select value={i18n.language.startsWith("ru") ? "ru" : "en"} onChange={(event) => i18n.changeLanguage(event.target.value)}><option value="ru">Русский</option><option value="en">English</option></select></label>;
}
