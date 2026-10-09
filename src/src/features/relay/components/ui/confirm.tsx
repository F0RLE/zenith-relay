import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "./buttons";
import { Dialog } from "./dialogs";
import { writeRelayPreference } from "../../state/relayPreferences";

type ConfirmOptions = {
  title?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  remember?: { key: string; label: string };
};
type ConfirmRequest = ConfirmOptions & { message: string };
type ConfirmHandler = (message: string, options?: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmHandler | null>(null);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [request, setRequest] = useState<ConfirmRequest | null>(null);
  const [remember, setRemember] = useState(false);
  const resolver = useRef<((accepted: boolean) => void) | null>(null);
  const requestRef = useRef<ConfirmRequest | null>(null);
  const confirm = useCallback<ConfirmHandler>((message, options = {}) => new Promise((resolve) => {
    resolver.current?.(false);
    resolver.current = resolve;
    requestRef.current = { message, ...options };
    setRemember(false);
    setRequest({ message, ...options });
  }), []);
  const settle = useCallback((accepted: boolean, savePreference = false) => {
    const resolve = resolver.current;
    if (savePreference && requestRef.current?.remember) {
      writeRelayPreference(requestRef.current.remember.key, "1");
    }
    resolver.current = null;
    requestRef.current = null;
    setRequest(null);
    resolve?.(accepted);
  }, []);
  useEffect(() => () => resolver.current?.(false), []);
  return <ConfirmContext.Provider value={confirm}>
    {children}
    {request ? <Dialog
      layer="top"
      className="confirm-dialog"
      title={request.title ?? t("common.confirmationTitle")}
      onClose={() => settle(false)}
      footer={<><Button variant="secondary" onClick={() => settle(false, remember)}>{request.cancelLabel ?? t("common.cancel")}</Button><Button variant={request.danger ? "danger" : "primary"} onClick={() => settle(true, remember)}>{request.confirmLabel ?? t("common.confirm")}</Button></>}
    >
      <p className="confirm-dialog-message">{request.message}</p>
      {request.remember ? <label className="confirm-dialog-remember">
        <input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />
        <span>{request.remember.label}</span>
      </label> : null}
    </Dialog> : null}
  </ConfirmContext.Provider>;
}

export function useConfirm() {
  const confirm = useContext(ConfirmContext);
  if (!confirm) throw new Error("ConfirmProvider is missing");
  return confirm;
}
