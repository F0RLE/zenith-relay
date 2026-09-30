import { useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import { editedLoginNotes, rememberAccountLoginDraft, type AccountLoginDraft, type LoginNoteEdits } from "../accountLoginDraft";
import { secondsUntil, useRelativeTimeClock } from "../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../hooks/useTransientFlag";
import { useRelayState } from "../state/RelayStateProvider";
import { Button, copyText } from "./Ui";

type AccountLoginNotesProps = {
  accountId?: string;
  loginId?: string;
};

export function AccountLoginNotes({ accountId, loginId }: AccountLoginNotesProps) {
  const { t } = useTranslation();
  const { refresh } = useRelayState();
  const [email, setEmail] = useState("");
  const [phone, setPhone] = useState("");
  const [password, setPassword] = useState("");
  const [totpSecret, setTotpSecret] = useState("");
  const [ready, setReady] = useState(!accountId);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const [preview, setPreview] = useState<{ code: string | null; expiresAtMs: number | null } | null>(null);
  const [savedVisible, showSaved] = useTransientFlag(1_500);
  const [codeCopied, showCodeCopied] = useTransientFlag(1_500);
  const savedEmail = useRef("");
  const savedValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const dirtyFields = useRef<LoginNoteEdits>({ email: false, phone: false, password: false, totpSecret: false });
  const loadedRef = useRef(false);
  const previewTimer = useRef(0);
  const latestValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const now = useRelativeTimeClock([preview?.expiresAtMs ?? null]);
  const remaining = preview?.expiresAtMs ? secondsUntil(preview.expiresAtMs, now) : 0;
  latestValues.current = { email, phone, password, totpSecret };

  useEffect(() => {
    if (!accountId) return;
    const currentAccountId = accountId;
    return () => {
      if (!loadedRef.current) return;
      const next = editedLoginNotes(savedValues.current, latestValues.current, dirtyFields.current);
      if (!next) return;
      savedValues.current = next;
      dirtyFields.current = { email: false, phone: false, password: false, totpSecret: false };
      void relayCommands.updateAccountLogin({ accountId: currentAccountId, ...next }).catch(() => undefined);
    };
  }, [accountId]);

  useEffect(() => {
    if (!accountId) return;
    let cancelled = false;
    void relayCommands.revealLocalAccountLogin(accountId).then((details) => {
      if (cancelled) return;
      const next = {
        email: details.email ?? "",
        phone: details.phone ?? "",
        password: details.password ?? "",
        totpSecret: details.totpSecret ?? "",
      };
      savedValues.current = next;
      savedEmail.current = next.email;
      dirtyFields.current = { email: false, phone: false, password: false, totpSecret: false };
      loadedRef.current = true;
      setEmail(next.email);
      setPhone(next.phone);
      setPassword(next.password);
      setTotpSecret(next.totpSecret);
      setReady(true);
    }).catch(() => {
      if (!cancelled) setLoadFailed(true);
    });
    return () => {
      cancelled = true;
    };
  }, [accountId]);

  useEffect(() => {
    if (!loginId || accountId) return;
    rememberAccountLoginDraft(loginId, { email, phone, password, totpSecret });
  }, [accountId, email, loginId, password, phone, totpSecret]);

  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      const next = await relayCommands.previewTotpCode(totpSecret);
      if (cancelled) return;
      setPreview(next);
      const delay = next.expiresAtMs ? Math.max(250, next.expiresAtMs - Date.now() + 50) : 30_000;
      previewTimer.current = window.setTimeout(() => void run(), delay);
    };
    if (!totpSecret.trim()) {
      setPreview(null);
      return;
    }
    void run();
    return () => {
      cancelled = true;
      window.clearTimeout(previewTimer.current);
    };
  }, [totpSecret]);

  const save = async () => {
    if (!accountId || !ready) return;
    const next = editedLoginNotes(savedValues.current, { email, phone, password, totpSecret }, dirtyFields.current);
    if (!next) return;
    try {
      await relayCommands.updateAccountLogin({ accountId, ...next });
      savedValues.current = next;
      dirtyFields.current = { email: false, phone: false, password: false, totpSecret: false };
      setSaveFailed(false);
      showSaved();
      if (next.email !== savedEmail.current) {
        savedEmail.current = next.email;
        void refresh();
      }
    } catch {
      setSaveFailed(true);
    }
  };

  const copyCode = async () => {
    if (!preview?.code) return;
    await copyText(preview.code);
    showCodeCopied();
  };

  return (
    <section className="account-login-notes" aria-label={t("accounts.loginDetails")}>
      {loadFailed ? <p role="alert" className="form-note error-text">{t("accounts.loginSaveFailed")}</p> : null}
      <div className="account-login-fields">
        <label className="relay-field">
          <span>{t("accounts.loginEmail")}</span>
          <input value={email} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => { dirtyFields.current.email = true; setEmail(event.target.value); }} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginPhone")}</span>
          <input value={phone} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => { dirtyFields.current.phone = true; setPhone(event.target.value); }} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginPassword")}</span>
          <input value={password} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => { dirtyFields.current.password = true; setPassword(event.target.value); }} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginTotpSecret")}</span>
          <input value={totpSecret} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => { dirtyFields.current.totpSecret = true; setTotpSecret(event.target.value); }} onBlur={() => void save()} />
        </label>
      </div>
      <div className="account-login-code">
        <span>{t("accounts.loginTotpCode")}</span>
        <strong>{preview?.code ?? "—"}</strong>
        {preview?.code ? <small>{t("accounts.loginCodeRemaining", { count: remaining })}</small> : null}
        {preview?.code ? (
          <Button
            className="account-login-copy"
            variant="secondary"
            data-copied={codeCopied ? "true" : "false"}
            aria-live="polite"
            icon={codeCopied ? <Check aria-hidden /> : <Copy aria-hidden />}
            onClick={() => void copyCode()}
          >
            {codeCopied ? t("feedback.copied") : t("common.copy")}
          </Button>
        ) : null}
      </div>
      {savedVisible ? <p role="status" className="form-note success-text">{t("accounts.loginSaved")}</p> : null}
      {saveFailed ? <p role="alert" className="form-note error-text">{t("accounts.loginSaveFailed")}</p> : null}
    </section>
  );
}
