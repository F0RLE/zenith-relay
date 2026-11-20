import { useEffect, useId, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import { editedLoginNotes, getAccountLoginDraft, rememberAccountLoginDraft, type AccountLoginDraft, type LoginNoteEdits } from "../accountLoginDraft";
import { secondsUntil, useRelativeTimeClock } from "../hooks/useRelativeTimeClock";
import { useTransientFlag } from "../hooks/useTransientFlag";
import { useTotpPreview } from "../hooks/useTotpPreview";
import { useRelayState } from "../state/RelayStateProvider";
import { Button, CopyButton, SecretField, copyText } from "./Ui";

type AccountLoginNotesProps = {
  accountId?: string;
  loginId?: string;
};

type PendingSave = {
  accountId: string;
  loginValues: AccountLoginDraft;
};

export function AccountLoginNotes({ accountId, loginId }: AccountLoginNotesProps) {
  const { t } = useTranslation();
  const { refresh, perform } = useRelayState();
  const emailId = useId();
  const notesRef = useRef<HTMLElement>(null);
  const initialDraft = useRef(loginId && !accountId ? getAccountLoginDraft(loginId) : undefined);
  const [email, setEmail] = useState(initialDraft.current?.email ?? "");
  const [phone, setPhone] = useState(initialDraft.current?.phone ?? "");
  const [password, setPassword] = useState(initialDraft.current?.password ?? "");
  const [totpSecret, setTotpSecret] = useState(initialDraft.current?.totpSecret ?? "");
  const [ready, setReady] = useState(!accountId);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);
  const { preview, refresh: refreshPreview } = useTotpPreview(totpSecret);
  const [codeCopied, showCodeCopied, clearCodeCopied] = useTransientFlag(1_500);
  const [copyFailed, setCopyFailed] = useState(false);
  const [copying, setCopying] = useState(false);
  const copyAttempt = useRef<object | null>(null);
  const savedEmail = useRef("");
  const savedValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const dirtyFields = useRef<LoginNoteEdits>({ email: false, phone: false, password: false, totpSecret: false });
  const loadedRef = useRef(false);
  const mountedRef = useRef(true);
  const saveInFlight = useRef<Promise<void> | null>(null);
  const activeSave = useRef<PendingSave | null>(null);
  const queuedSaves = useRef<PendingSave[]>([]);
  const accountIdRef = useRef(accountId);
  const enqueueSaveRef = useRef<(targetAccountId?: string) => void>(() => undefined);
  const latestValues = useRef<AccountLoginDraft>({ email: "", phone: "", password: "", totpSecret: "" });
  const now = useRelativeTimeClock([preview?.expiresAtMs ?? null]);
  const remaining = preview?.expiresAtMs ? secondsUntil(preview.expiresAtMs, now) : 0;
  accountIdRef.current = accountId;
  latestValues.current = { email, phone, password, totpSecret };
  const updateDraftField = (field: keyof AccountLoginDraft, fieldValue: string, setFieldValue: (value: string) => void) => {
    dirtyFields.current[field] = true;
    latestValues.current = { ...latestValues.current, [field]: fieldValue };
    if (loginId && !accountId) rememberAccountLoginDraft(loginId, latestValues.current);
    setFieldValue(fieldValue);
  };

  const persistQueuedSaves = async () => {
    while (queuedSaves.current.length > 0) {
      const pending = queuedSaves.current.shift()!;
      activeSave.current = pending;
      try {
        await relayCommands.updateAccountLogin({ accountId: pending.accountId, ...pending.loginValues });
      } catch {
        // Retry the latest queued version for this account, not the failed snapshot.
        if (!queuedSaves.current.some((save) => save.accountId === pending.accountId)) {
          queuedSaves.current.unshift(pending);
        }
        if (mountedRef.current && accountIdRef.current === pending.accountId) {
          setSaveFailed(true);
        }
        return;
      } finally {
        activeSave.current = null;
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
      }
      if (pending.loginValues.email !== savedEmail.current) {
        savedEmail.current = pending.loginValues.email;
        if (mountedRef.current) void perform("account-notes-refresh", refresh, undefined, { skipRefresh: true, uiLock: false });
      }
    }
  };

  const enqueueCurrentSave = (targetAccountId = accountId) => {
    if (!targetAccountId || !ready || !loadedRef.current) return;
    const existing = queuedSaves.current.find((pending) => pending.accountId === targetAccountId);
    const baseline = existing?.loginValues
      ?? (activeSave.current?.accountId === targetAccountId ? activeSave.current.loginValues : savedValues.current);
    const changedValues = editedLoginNotes(baseline, latestValues.current, dirtyFields.current);
    if (changedValues) {
      if (existing) existing.loginValues = changedValues;
      else queuedSaves.current.push({ accountId: targetAccountId, loginValues: changedValues });
    } else if (!existing) {
      return;
    }
    if (!saveInFlight.current) {
      const task = persistQueuedSaves();
      saveInFlight.current = task;
      void task.finally(() => {
        if (saveInFlight.current === task) saveInFlight.current = null;
      });
    }
  };
  enqueueSaveRef.current = (targetAccountId = accountId) => enqueueCurrentSave(targetAccountId);

  useEffect(() => {
    if (!accountId) return;
    const currentAccountId = accountId;
    return () => {
      if (loadedRef.current) enqueueSaveRef.current(currentAccountId);
      loadedRef.current = false;
      // The queued command still runs after unmount, but it must not update
      // React state in a closed dialog.
    };
  }, [accountId]);

  useEffect(() => {
    mountedRef.current = true;
    const keyboardNavigation = (event: KeyboardEvent) => {
      if (event.key === "Tab") notesRef.current?.removeAttribute("data-pointer-edit");
    };
    document.addEventListener("keydown", keyboardNavigation, true);
    return () => {
      mountedRef.current = false;
      document.removeEventListener("keydown", keyboardNavigation, true);
    };
  }, []);

  useEffect(() => {
    if (!accountId) return;
    let cancelled = false;
    setReady(false);
    setLoadFailed(false);
    setSaveFailed(false);
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
    copyAttempt.current = null;
    setCopying(false);
    setCopyFailed(false);
    clearCodeCopied();
  }, [totpSecret, clearCodeCopied]);

  const save = () => enqueueSaveRef.current();

  const copyCode = async () => {
    if (copyAttempt.current) return;
    const attempt = {};
    copyAttempt.current = attempt;
    const current = () => mountedRef.current && copyAttempt.current === attempt && latestValues.current.totpSecret === totpSecret;
    setCopying(true);
    setCopyFailed(false);
    clearCodeCopied();
    try {
      const fresh = await refreshPreview();
      if (!current()) return;
      if (!fresh?.code || fresh.expiresAtMs == null || fresh.expiresAtMs <= Date.now()) {
        setCopyFailed(true);
        return;
      }
      await copyText(fresh.code);
      if (current()) showCodeCopied();
    } catch {
      if (current()) setCopyFailed(true);
    } finally {
      if (current()) {
        copyAttempt.current = null;
        setCopying(false);
      }
    }
  };

  return (
    <section ref={notesRef} className="account-login-notes" aria-label={t("accounts.loginDetails")}
      onPointerDownCapture={(event) => event.currentTarget.setAttribute("data-pointer-edit", "")}>
      <div className="account-login-fields">
        <div className="relay-field">
          <label htmlFor={emailId}>{t("accounts.loginEmail")}</label>
          <div className="secret-field">
            <input id={emailId} value={email} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("email", event.target.value, setEmail)} onBlur={() => void save()} />
            <div className="secret-field-actions">
              <CopyButton value={email} label={`${t("common.copy")}: ${t("accounts.loginEmail")}`} disabled={!ready || !email.trim()} preserveFocus />
            </div>
          </div>
        </div>
        <SecretField
          key={`${accountId ?? loginId}:password`}
          label={t("accounts.loginPassword")}
          value={password}
          disabled={!ready}
          onChange={(value) => updateDraftField("password", value, setPassword)}
          onBlur={() => void save()}
        />
        <label className="relay-field">
          <span>{t("accounts.loginPhone")}</span>
          <input type="tel" value={phone} disabled={!ready} autoComplete="off" spellCheck={false} onChange={(event) => updateDraftField("phone", event.target.value, setPhone)} onBlur={() => void save()} />
        </label>
        <SecretField
          key={`${accountId ?? loginId}:totp`}
          label={t("accounts.loginTotpSecret")}
          value={totpSecret}
          disabled={!ready}
          copyable={false}
          onChange={(value) => updateDraftField("totpSecret", value, setTotpSecret)}
          onBlur={() => void save()}
        />
      </div>
      {totpSecret.trim() ? <div className="account-login-code">
        <span>{t("accounts.loginTotpCode")}</span>
        <strong>{preview?.code ?? "—"}</strong>
        {preview?.code ? <small>{t("accounts.loginCodeRemaining", { count: remaining })}</small> : null}
        {preview?.code ? (
          <Button
            className="account-login-copy"
            variant="secondary"
            disabled={copying}
            data-copied={codeCopied ? "true" : "false"}
            title={copyFailed ? t("common.copyFailed") : undefined}
            aria-label={copyFailed ? `${t("common.copy")}: ${t("common.copyFailed")}` : undefined}
            aria-live="polite"
            icon={codeCopied ? <Check aria-hidden /> : <Copy aria-hidden />}
            onClick={() => void copyCode()}
          >
            {codeCopied ? t("feedback.copied") : t("common.copy")}
          </Button>
        ) : null}
      </div> : null}
      {loadFailed || saveFailed ? <div className="account-login-feedback">
        <p role="alert" className="form-note error-text">{t(loadFailed ? "accounts.loginLoadFailed" : "accounts.loginSaveFailed")}</p>
      </div> : null}
    </section>
  );
}
