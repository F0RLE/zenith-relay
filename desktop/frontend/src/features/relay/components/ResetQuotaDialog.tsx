import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Dialog } from "./Ui";

export function ResetQuotaDialog({ onClose, onConfirm }: { onClose: () => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  const [remainingSeconds, setRemainingSeconds] = useState(3);
  useEffect(() => {
    const deadline = performance.now() + 3_000;
    const timer = window.setInterval(() => {
      const remaining = Math.max(0, Math.ceil((deadline - performance.now()) / 1_000));
      setRemainingSeconds(remaining);
      if (remaining === 0) window.clearInterval(timer);
    }, 100);
    return () => window.clearInterval(timer);
  }, []);
  return <Dialog
    layer="top"
    className="confirm-dialog-compact"
    title={t("accounts.resetCreditsConfirm")}
    onClose={onClose}
    footer={<>
      <Button variant="secondary" onClick={onClose}>{t("accounts.resetCreditsConfirmNo")}</Button>
      <Button variant="primary" disabled={remainingSeconds > 0} onClick={() => { if (remainingSeconds === 0) onConfirm(); }}>
        <span aria-live="polite" aria-atomic="true">
          {t("accounts.resetCreditsConfirmYes")}{remainingSeconds > 0 ? ` (${remainingSeconds})` : ""}
        </span>
      </Button>
    </>}
  >{null}</Dialog>;
}
