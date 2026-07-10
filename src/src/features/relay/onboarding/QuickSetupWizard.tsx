import { useState } from "react";
import { ArrowLeft, Check, Cloud, Laptop, Server, SkipForward } from "lucide-react";
import { useTranslation } from "react-i18next";
import { saveKey } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { OAuthFlow, RelayMode } from "../api/types";
import { Button, SecretField } from "../components/Ui";
import { useRelayState } from "../state/RelayStateProvider";

export function QuickSetupWizard() {
  const { t, i18n } = useTranslation();
  const { finishOnboarding, perform, busy } = useRelayState();
  const [intro, setIntro] = useState(true);
  const [step, setStep] = useState(1);
  const [mode, setMode] = useState<RelayMode>("local");
  const [client, setClient] = useState("later");
  const [apiKey, setApiKey] = useState("");
  const [serverUrl, setServerUrl] = useState("");
  const [serverToken, setServerToken] = useState("");
  const [oauth, setOauth] = useState<OAuthFlow | null>(null);
  const [connectionReady, setConnectionReady] = useState(false);

  if (intro) {
    return <main className="setup-shell"><header className="setup-top"><strong>Zenith Relay</strong><LanguageSelect /></header><section className="product-intro"><div className="intro-copy"><h1>Zenith Relay</h1><p>{t("onboarding.intro")}</p><div className="flow-line" aria-label={t("onboarding.flowLabel")}><span>{t("onboarding.accounts")}</span><i /><strong>Zenith Relay</strong><i /><span>{t("onboarding.apps")}</span></div><dl><div><dt>{t("modes.local")}</dt><dd>{t("onboarding.factLocal")}</dd></div><div><dt>{t("modes.remote")}</dt><dd>{t("onboarding.factRemote")}</dd></div><div><dt>{t("modes.zenith")}</dt><dd>{t("onboarding.factReady")}</dd></div></dl></div><div className="intro-actions"><Button variant="primary" onClick={() => setIntro(false)}>{t("onboarding.start")}</Button><Button variant="ghost" icon={<SkipForward aria-hidden />} onClick={() => finishOnboarding("local")}>{t("onboarding.skip")}</Button></div></section></main>;
  }

  const canContinue = step !== 2 || (mode === "local" ? connectionReady : mode === "remote" ? Boolean(serverUrl && serverToken) : Boolean(apiKey));
  const next = async () => {
    if (step === 2 && mode === "remote") {
      const ok = await perform("onboarding-remote", () => relayCommands.connectRemote({ baseUrl: serverUrl, managementToken: serverToken, allowInsecureHttp: serverUrl.startsWith("http://"), confirmIdentityChange: false }), "feedback.connected");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 2 && mode === "zenith") {
      const ok = await perform("onboarding-ready", () => saveKey(apiKey), "feedback.saved");
      if (!ok) return;
      setConnectionReady(true);
    }
    if (step === 5) finishOnboarding(mode);
    else setStep((value) => value + 1);
  };

  return <main className="setup-shell"><header className="setup-top"><strong>Zenith Relay</strong><LanguageSelect /></header><ol className="setup-progress" aria-label={t("onboarding.progress")}>{[1,2,3,4,5].map((value) => <li key={value} className={value <= step ? "active" : ""}><span>{value < step ? <Check aria-hidden /> : value}</span>{t(`onboarding.steps.${value}`)}</li>)}</ol><section className="setup-body">
    {step === 1 ? <><h1>{t("onboarding.modeQuestion")}</h1><p>{t("onboarding.modeHint")}</p><div className="mode-options">{(["local","remote","zenith"] as RelayMode[]).map((value) => { const Icon = value === "local" ? Laptop : value === "remote" ? Server : Cloud; return <button key={value} type="button" className={mode === value ? "selected" : ""} onClick={() => { setMode(value); setConnectionReady(false); }}><Icon aria-hidden /><span><strong>{t(`modes.${value}`)}</strong><small>{t(`onboarding.modeDescriptions.${value}`)}</small></span>{mode === value ? <Check aria-hidden /> : null}</button>; })}</div></> : null}
    {step === 2 ? <ConnectionStep mode={mode} apiKey={apiKey} setApiKey={setApiKey} serverUrl={serverUrl} setServerUrl={setServerUrl} serverToken={serverToken} setServerToken={setServerToken} oauth={oauth} setOauth={setOauth} onConnected={() => setConnectionReady(true)} /> : null}
    {step === 3 ? <><h1>{t("onboarding.checkTitle")}</h1><p>{t("onboarding.checkHint")}</p><ul className="check-stages">{["credentials","endpoint","models","capacity"].map((value) => <li key={value}><Check aria-hidden /><span>{t(`onboarding.checks.${value}`)}</span><strong>{t("common.ready")}</strong></li>)}</ul></> : null}
    {step === 4 ? <><h1>{t("onboarding.clientQuestion")}</h1><p>{t("onboarding.clientHint")}</p><div className="client-options">{["codex","opencode","other","later"].map((value) => <button type="button" key={value} className={client === value ? "selected" : ""} onClick={() => setClient(value)}>{t(`clients.${value}`)}{client === value ? <Check aria-hidden /> : null}</button>)}</div></> : null}
    {step === 5 ? <div className="setup-ready"><Check aria-hidden /><h1>{t("onboarding.readyTitle")}</h1><p>{t("onboarding.readyHint", { mode: t(`modes.${mode}`), client: t(`clients.${client}`) })}</p></div> : null}
  </section><footer className="setup-footer"><Button variant="ghost" icon={<ArrowLeft aria-hidden />} disabled={step === 1} onClick={() => setStep((value) => Math.max(1, value - 1))}>{t("common.back")}</Button><Button variant="primary" busy={busy?.startsWith("onboarding") ?? false} disabled={!canContinue} onClick={next}>{step === 5 ? t("onboarding.openApp") : t("common.continue")}</Button></footer></main>;
}

function ConnectionStep({ mode, apiKey, setApiKey, serverUrl, setServerUrl, serverToken, setServerToken, oauth, setOauth, onConnected }: { mode: RelayMode; apiKey: string; setApiKey: (value: string) => void; serverUrl: string; setServerUrl: (value: string) => void; serverToken: string; setServerToken: (value: string) => void; oauth: OAuthFlow | null; setOauth: (value: OAuthFlow | null) => void; onConnected: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  if (mode === "remote") return <><h1>{t("onboarding.connectionRemote")}</h1><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={serverUrl} onChange={(event) => setServerUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={serverToken} onChange={setServerToken} /></>;
  if (mode === "zenith") return <><h1>{t("onboarding.connectionReady")}</h1><div className="recommended-line"><strong>Zenith API</strong><span>{t("common.recommended")}</span></div><SecretField label={t("readyApi.key")} value={apiKey} onChange={setApiKey} /></>;
  return <><h1>{t("onboarding.connectionLocal")}</h1><p>{t("onboarding.oauthHint")}</p>{oauth ? <div className="oauth-progress"><strong>{t("accounts.browserOpened")}</strong><code>{oauth.redirectUri}</code><Button variant="primary" busy={busy === "oauth-complete"} onClick={async () => { const ok = await perform("oauth-complete", () => relayCommands.completeOAuth(oauth.loginId), "feedback.accountAdded"); if (ok) onConnected(); }}>{t("accounts.finishSignIn")}</Button><Button variant="ghost" onClick={() => perform("oauth-cancel", () => relayCommands.cancelOAuth(oauth.loginId)).then(() => setOauth(null))}>{t("common.cancel")}</Button></div> : <Button variant="primary" busy={busy === "oauth-start"} onClick={async () => { const result: { current: OAuthFlow | null } = { current: null }; const ok = await perform("oauth-start", async () => { result.current = await relayCommands.startOAuth(); }); if (ok) setOauth(result.current); }}>{t("accounts.signIn")}</Button>}<p className="form-note">{t("onboarding.importLater")}</p></>;
}

function LanguageSelect() {
  const { i18n, t } = useTranslation();
  return <label className="language-select"><span>{t("settings.language")}</span><select value={i18n.language.startsWith("ru") ? "ru" : "en"} onChange={(event) => i18n.changeLanguage(event.target.value)}><option value="ru">Русский</option><option value="en">English</option></select></label>;
}
