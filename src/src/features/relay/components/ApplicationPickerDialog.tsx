import { useId } from "react";
import { useTranslation } from "react-i18next";
import { Dialog } from "./Ui";

type ApplicationPickerDialogProps = {
  title?: string;
  onClose: () => void;
  onChatGPT: () => void;
  onOpenCode: () => void;
  chatGPTDisabled?: boolean;
};

export function ApplicationPickerDialog({
  title,
  onClose,
  onChatGPT,
  onOpenCode,
  chatGPTDisabled = false,
}: ApplicationPickerDialogProps) {
  const { t } = useTranslation();
  const unavailableDescriptionId = useId();
  const dialogTitle = title ?? t("pool.connectDialogTitle");
  const choose = (action: () => void) => {
    onClose();
    action();
  };
  return <Dialog className="application-picker" title={dialogTitle} onClose={onClose}>
    <div className="application-picker-options">
      <button
        type="button"
        className="application-picker-option"
        disabled={chatGPTDisabled}
        aria-label={t("pool.connectChatGPT")}
        aria-describedby={chatGPTDisabled ? unavailableDescriptionId : undefined}
        onClick={() => choose(onChatGPT)}
      >
        <img className="application-picker-logo chatgpt" src="/icons/chatgpt.svg" alt="" />
        <span className="application-picker-label">
          <strong>{t("pool.connectChatGPT")}</strong>
          {chatGPTDisabled ? <small id={unavailableDescriptionId}>{t("sources.launchResponsesOnly")}</small> : null}
        </span>
      </button>
      <button type="button" className="application-picker-option" onClick={() => choose(onOpenCode)}>
        <img className="application-picker-logo" src="/icons/opencode.svg" alt="" />
        <span className="application-picker-label"><strong>{t("pool.connectOpenCode")}</strong></span>
      </button>
    </div>
  </Dialog>;
}
