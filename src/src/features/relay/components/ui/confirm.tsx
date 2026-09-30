import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "./buttons";
import { Dialog } from "./dialogs";

type ConfirmOptions = { title?: string; confirmLabel?: string; cancelLabel?: string; danger?: boolean };
type ConfirmRequest = ConfirmOptions & { message: string };
type ConfirmHandler = (message: string, options?: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmHandler | null>(null);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [request, setRequest] = useState<ConfirmRequest | null>(null);
  const resolver = useRef<((accepted: boolean) => void) | null>(null);
  const confirm = useCallback<ConfirmHandler>((message, options = {}) => new Promise((resolve) => {
    resolver.current?.(false);
    resolver.current = resolve;
    setRequest({ message, ...options });
  }), []);
  const settle = useCallback((accepted: boolean) => {
    const resolve = resolver.current;
    resolver.current = null;
    setRequest(null);
    resolve?.(accepted);
  }, []);
  useEffect(() => () => resolver.current?.(false), []);
  return <ConfirmContext.Provider value={confirm}>
    {children}
    {request ? <Dialog
      title={request.title ?? t("common.confirmationTitle")}
      onClose={() => settle(false)}
      footer={<><Button variant="secondary" onClick={() => settle(false)}>{request.cancelLabel ?? t("common.cancel")}</Button><Button variant={request.danger ? "danger" : "primary"} onClick={() => settle(true)}>{request.confirmLabel ?? t("common.confirm")}</Button></>}
    ><p className="confirm-dialog-message">{request.message}</p></Dialog> : null}
  </ConfirmContext.Provider>;
}

export function useConfirm() {
  const confirm = useContext(ConfirmContext);
  if (!confirm) throw new Error("ConfirmProvider is missing");
  return confirm;
}
