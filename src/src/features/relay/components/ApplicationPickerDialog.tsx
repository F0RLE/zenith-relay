import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Dialog, ToggleSwitch } from "./Ui";
import { readLaunchApplicationAfterConnect, writeLaunchApplicationAfterConnect } from "../state/relayPreferences";

type ApplicationPickerDialogProps = {
  title?: string;
  onClose: () => void;
  onChatGPT: (launchAfterConnect: boolean) => void;
  onOpenCode: (launchAfterConnect: boolean) => void;
  showLaunchToggle?: boolean;
  chatGPTDisabled?: boolean;
};

/** Shared application picker used by Pool and Overview actions. */
export function ApplicationPickerDialog({
  title,
  onClose,
  onChatGPT,
  onOpenCode,
  showLaunchToggle = true,
  chatGPTDisabled = false,
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
      <button type="button" className="pool-connection-option" disabled={chatGPTDisabled} data-relay-tooltip={chatGPTDisabled ? t("sources.launchResponsesOnly") : undefined} onClick={() => choose(() => onChatGPT(launchAfterConnect))}>
        <span className="pool-connection-option-icon"><img src="/icons/chatgpt.svg" alt="" /></span>
        <strong>{t("pool.connectChatGPT")}</strong>
      </button>
      <button type="button" className="pool-connection-option" onClick={() => choose(() => onOpenCode(launchAfterConnect))}>
        <span className="pool-connection-option-icon"><img src="/icons/opencode.svg" alt="" /></span>
        <strong>{t("pool.connectOpenCode")}</strong>
      </button>
    </div>
    {showLaunchToggle ? <label className="pool-connection-launch-toggle">
      <ToggleSwitch label={t("pool.launchAfterConnect")} checked={launchAfterConnect} onChange={(enabled) => {
        setLaunchAfterConnect(enabled);
        writeLaunchApplicationAfterConnect(enabled);
      }} />
      <span>{t("pool.launchAfterConnect")}</span>
    </label> : null}
  </Dialog>;
}
