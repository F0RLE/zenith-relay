import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Dialog } from "./Ui";
import { readLaunchApplicationAfterConnect, writeLaunchApplicationAfterConnect } from "../state/relayPreferences";

type ApplicationPickerDialogProps = {
  title?: string;
  onClose: () => void;
  onChatGPT: (launchAfterConnect: boolean) => void;
  onOpenCode: (launchAfterConnect: boolean) => void;
  showLaunchToggle?: boolean;
};

/** Shared application picker used by Pool and Overview actions. */
export function ApplicationPickerDialog({
  title,
  onClose,
  onChatGPT,
  onOpenCode,
  showLaunchToggle = true,
}: ApplicationPickerDialogProps) {
  const { t } = useTranslation();
  const [launchAfterConnect, setLaunchAfterConnect] = useState(readLaunchApplicationAfterConnect);
  const dialogTitle = title ?? t("pool.connectDialogTitle");
  const choose = (action: () => void) => {
    onClose();
    action();
  };
  return <Dialog className="pool-connection-picker" title={dialogTitle} onClose={onClose}>
    <div className="pool-connection-options" role="list" aria-label={dialogTitle}>
      <button type="button" className="pool-connection-option" onClick={() => choose(() => onChatGPT(launchAfterConnect))}>
        <span className="pool-connection-option-icon"><img src="/icons/chatgpt.svg" alt="" /></span>
        <strong>{t("pool.connectChatGPT")}</strong>
      </button>
      <button type="button" className="pool-connection-option" onClick={() => choose(() => onOpenCode(launchAfterConnect))}>
        <span className="pool-connection-option-icon"><img src="/icons/opencode.svg" alt="" /></span>
        <strong>{t("pool.connectOpenCode")}</strong>
      </button>
    </div>
    {showLaunchToggle ? <label className="pool-connection-launch-toggle">
      <input type="checkbox" checked={launchAfterConnect} onChange={(event) => {
        const enabled = event.target.checked;
        setLaunchAfterConnect(enabled);
        writeLaunchApplicationAfterConnect(enabled);
      }} />
      <span className="pool-connection-launch-switch" aria-hidden="true"><span /></span>
      <span>{t("pool.launchAfterConnect")}</span>
    </label> : null}
  </Dialog>;
}
