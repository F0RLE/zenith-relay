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

type PendingSave = {
  accountId: string;
  loginValues: AccountLoginDraft;
  notify: boolean;
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
  const [savedVisible, showSaved, hideSaved] = useTransientFlag(1_500);
  const [codeCopied, showCodeCopied] = useTransientFlag(1_500);
  const savedEmail = useRef("");
  const savedValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const dirtyFields = useRef<LoginNoteEdits>({ email: false, phone: false, password: false, totpSecret: false });
  const loadedRef = useRef(false);
  const mountedRef = useRef(true);
  const saveInFlight = useRef<Promise<void> | null>(null);
  const queuedSaves = useRef<PendingSave[]>([]);
  const accountIdRef = useRef(accountId);
  const enqueueSaveRef = useRef<(notify?: boolean, targetAccountId?: string) => void>(() => undefined);
  const previewTimer = useRef(0);
  const latestValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const now = useRelativeTimeClock([preview?.expiresAtMs ?? null]);
  const remaining = preview?.expiresAtMs ? secondsUntil(preview.expiresAtMs, now) : 0;
  accountIdRef.current = accountId;
  latestValues.current = { email, phone, password, totpSecret };
  const updateDraftField = (field: keyof AccountLoginDraft, fieldValue: string, setFieldValue: (value: string) => void) => {
    dirtyFields.current[field] = true;
    latestValues.current = { ...latestValues.current, [field]: fieldValue };
    setFieldValue(fieldValue);
  };

  const persistQueuedSaves = async () => {
    while (queuedSaves.current.length > 0) {
      const pending = queuedSaves.current.shift()!;
      try {
        await relayCommands.updateAccountLogin({ accountId: pending.accountId, ...pending.loginValues });
      } catch {
        // Keep the values dirty so the next blur can retry them. Never let a
        // failed background save look like a successful one.
        queuedSaves.current.unshift(pending);
        if (mountedRef.current && accountIdRef.current === pending.accountId) {
          hideSaved();
          setSaveFailed(true);
        }
        return;
      }
      if (accountIdRef.current !== pending.accountId) continue;
      const latest = latestValues.current;
      savedValues.current = pending.loginValues;
      dirtyFields.current = {
        email: latest.email !== pending.loginValues.email,
        phone: latest.phone !== pending.loginValues.phone,
        password: latest.password !== pending.loginValues.password,
        totpSecret: latest.totpSecret !== pending.loginValues.totpSecret,
      };
      if (mountedRef.current) {
        setSaveFailed(false);
        if (pending.notify) showSaved();
      }
      if (pending.loginValues.email !== savedEmail.current) {
        savedEmail.current = pending.loginValues.email;
        if (mountedRef.current) void refresh();
      }
    }
  };

  const enqueueCurrentSave = (notify = true, targetAccountId = accountId) => {
    if (!targetAccountId || !ready || !loadedRef.current) return;
    const changedValues = editedLoginNotes(savedValues.current, latestValues.current, dirtyFields.current);
    if (!changedValues) return;
    hideSaved();
    const existing = queuedSaves.current.find((pending) => pending.accountId === targetAccountId);
    const pending: PendingSave = {
      accountId: targetAccountId,
      loginValues: changedValues,
      notify: Boolean(existing?.notify || notify),
    };
    if (existing) {
      existing.loginValues = pending.loginValues;
      existing.notify = pending.notify;
    } else {
      queuedSaves.current.push(pending);
    }
    if (!saveInFlight.current) {
      const task = persistQueuedSaves();
      saveInFlight.current = task;
      void task.finally(() => {
        if (saveInFlight.current === task) saveInFlight.current = null;
      });
    }
  };
  enqueueSaveRef.current = (notify = true, targetAccountId = accountId) => enqueueCurrentSave(notify, targetAccountId);

  useEffect(() => {
    if (!accountId) return;
    const currentAccountId = accountId;
    return () => {
      if (loadedRef.current) enqueueSaveRef.current(false, currentAccountId);
      loadedRef.current = false;
      // The queued command still runs after unmount, but it must not update
      // React state or show a false "Saved" status in a closed dialog.
    };
  }, [accountId]);

  useEffect(() => () => {
    mountedRef.current = false;
  }, []);

  useEffect(() => {
    if (!accountId) return;
    let cancelled = false;
    setReady(false);
    setLoadFailed(false);
    setSaveFailed(false);
    hideSaved();
    setEmail("");
    setPhone("");
    setPassword("");
    setTotpSecret("");
    void relayCommands.revealLocalAccountLogin(accountId).then((details) => {
      if (cancelled) return;
      setLoadFailed(false);
      setSaveFailed(false);
      const loadedValues = {
        email: details.email ?? "",
        phone: details.phone ?? "",
        password: details.password ?? "",
        totpSecret: details.totpSecret ?? "",
      };
      savedValues.current = loadedValues;
      savedEmail.current = loadedValues.email;
      dirtyFields.current = { email: false, phone: false, password: false, totpSecret: false };
      loadedRef.current = true;
      setEmail(loadedValues.email);
      setPhone(loadedValues.phone);
      setPassword(loadedValues.password);
      setTotpSecret(loadedValues.totpSecret);
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
      const totpPreview = await relayCommands.previewTotpCode(totpSecret);
      if (cancelled) return;
      setPreview(totpPreview);
      const delay = totpPreview.expiresAtMs ? Math.max(250, totpPreview.expiresAtMs - Date.now() + 50) : 30_000;
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

  const save = () => enqueueSaveRef.current();

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
          <input value={email} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("email", event.target.value, setEmail)} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginPhone")}</span>
          <input value={phone} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("phone", event.target.value, setPhone)} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginPassword")}</span>
          <input value={password} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("password", event.target.value, setPassword)} onBlur={() => void save()} />
        </label>
        <label className="relay-field">
          <span>{t("accounts.loginTotpSecret")}</span>
          <input value={totpSecret} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("totpSecret", event.target.value, setTotpSecret)} onBlur={() => void save()} />
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
