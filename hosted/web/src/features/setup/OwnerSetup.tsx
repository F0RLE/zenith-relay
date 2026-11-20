import { useRef, useState } from "react";
import { getAssertion } from "../../api/ownerSession";
import { LanguageSwitcher } from "../../components/LanguageSwitcher";
import { RelayMark } from "../../components/RelayMark";
import { useTheme } from "../../components/useTheme";
import { useLocale } from "../../i18n/LocaleProvider";

type Proof = { token: string; options: PublicKeyCredentialRequestOptionsJSON };

async function send<T>(step: "start" | "register" | "prove", token: string, data: unknown): Promise<T> {
  const r = await fetch(`/api/hosted/v1/setup/${step}`, {
    method: "POST", credentials: "omit", mode: "same-origin", redirect: "error", cache: "no-store",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify(data), signal: AbortSignal.timeout(10000),
  });
  if (!r.ok || !r.headers.get("Content-Type")?.startsWith("application/json")) throw new Error("Setup unavailable");
  const result = await r.json();
  if (result?.meta?.api_version !== "1" || !result.data) throw new Error("Incompatible setup API");
  return result.data;
}

export function OwnerSetup() {
  const { t } = useLocale();
  const { theme } = useTheme();
  const code = useRef<HTMLInputElement>(null);
  const [proof, setProof] = useState<Proof>();
  const [phase, setPhase] = useState<"ready" | "busy" | "done" | "failed">("ready");
  const running = useRef(false);
  async function proceed() {
    if (running.current) return;
    running.current = true;
    setPhase("busy");
    try {
      if (!proof) {
        const token = code.current?.value.trim();
        if (code.current) code.current.value = "";
        if (!token) throw new Error("Missing setup code");
        const begin = await send<{ session: string; options: { publicKey: PublicKeyCredentialCreationOptionsJSON } }>("start", token, {});
        const key = await navigator.credentials.create({ publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(begin.options.publicKey) });
        if (!(key instanceof PublicKeyCredential)) throw new Error("No passkey");
        const options = await send<{ publicKey: PublicKeyCredentialRequestOptionsJSON }>("register", begin.session, key.toJSON());
        setProof({ token: begin.session, options: options.publicKey });
        setPhase("ready");
      } else {
        const key = await getAssertion(proof.options);
        setProof(undefined);
        const saved = await send<{ owner_bound: boolean }>("prove", proof.token, key);
        if (saved.owner_bound !== true) throw new Error("Owner not confirmed");
        setPhase("done");
      }
    } catch {
      setProof(undefined);
      setPhase("failed");
    } finally {
      running.current = false;
    }
  }
  return <div className="relay-app landing hosted-entry" data-theme={theme} style={{ colorScheme: theme }}>
    <header className="hosted-entry-header"><span className="landing-brand"><RelayMark className="landing-mark" /><strong>Relay</strong></span><LanguageSwitcher /></header>
    <main className="integration-unavailable hosted-entry-content">
      <h1>{t("setup.owner")}</h1>
      {phase === "done" ? <p role="status">{t("setup.saved")}</p> : phase === "failed" ? <p role="alert">{t("setup.failed")}</p>
        : <form className="owner-setup-form" onSubmit={event => { event.preventDefault(); void proceed(); }} aria-busy={phase === "busy"}>
          {!proof && <label>{t("setup.code")}<input ref={code} type="password" autoComplete="off" spellCheck={false} required maxLength={64} disabled={phase === "busy"} /></label>}
          <button className="key-button key-primary" type="submit" disabled={phase === "busy"}>{t(proof ? "setup.confirm" : "setup.register")}</button>
        </form>}
    </main>
  </div>;
}
