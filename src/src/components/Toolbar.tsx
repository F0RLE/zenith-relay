import { CheckCircle2, Eye, EyeOff, Play, Save } from "lucide-react";
import type { FormEvent } from "react";
import { useTranslation } from "react-i18next";

type ToolbarProps = {
  apiKey: string;
  canLaunch: boolean;
  canSave: boolean;
  codexRunning: boolean;
  keyVisible: boolean;
  saved: boolean;
  onApiKeyChange: (value: string) => void;
  onKeyVisibleChange: (value: boolean) => void;
  onLaunch: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
};

export function Toolbar({
  apiKey,
  canLaunch,
  canSave,
  codexRunning,
  keyVisible,
  saved,
  onApiKeyChange,
  onKeyVisibleChange,
  onLaunch,
  onSubmit,
}: ToolbarProps) {
  const { t } = useTranslation();

  return (
    <form className="toolbar" onSubmit={onSubmit}>
      <label className="api-field">
        <span className="sr-only">{t("apiKey.label")}</span>
        <input
          value={apiKey}
          onChange={(event) => onApiKeyChange(event.target.value)}
          type={keyVisible ? "text" : "password"}
          autoComplete="off"
          spellCheck={false}
          placeholder={t("apiKey.label")}
        />
        <button
          className="icon-button"
          type="button"
          title={keyVisible ? t("apiKey.hide") : t("apiKey.show")}
          aria-label={keyVisible ? t("apiKey.hide") : t("apiKey.show")}
          onClick={() => onKeyVisibleChange(!keyVisible)}
        >
          {keyVisible ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
        </button>
      </label>

      <button
        className={`save-button ${saved ? "saved" : ""}`}
        type="submit"
        disabled={!canSave}
        title={saved ? t("actions.saved") : t("actions.save")}
        aria-label={saved ? t("actions.saved") : t("actions.save")}
      >
        <Save aria-hidden />
      </button>

      <button className={`launch-button ${codexRunning ? "running" : ""}`} type="button" disabled={!canLaunch} onClick={onLaunch}>
        {codexRunning ? <CheckCircle2 aria-hidden /> : <Play aria-hidden />}
        <span>{codexRunning ? t("actions.running") : t("actions.launch")}</span>
      </button>
    </form>
  );
}
