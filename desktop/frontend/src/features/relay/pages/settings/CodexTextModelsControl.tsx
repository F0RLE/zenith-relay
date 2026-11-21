import { useEffect, useId, useRef, useState } from "react";
import { TriangleAlert } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, Dialog, SettingToggle } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { useSavedChoice } from "../../state/usePendingFlag";

const CONFIRM_DELAY_MS = 10_000;

export function CodexTextModelsControl() {
  const { t } = useTranslation();
  const { runtime, perform } = useRelayState();
  const saved = runtime?.gateway.codexTextModelsEnabled ?? false;
  const choice = useSavedChoice(saved);
  const [confirming, setConfirming] = useState(false);
  const [saving, setSaving] = useState(false);
  const inFlight = useRef(false);
  const available = runtime?.capabilities.features.includes("codex_text_models") ?? false;
  const saveTextModelAccess = async (enabled: boolean) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    try {
      const ok = await perform(
        "codex-text-models",
        () => relayCommands.setCodexTextModels(enabled, choice.value, enabled),
        "feedback.saved",
        { backgroundRefresh: true, uiLock: false },
      );
      if (ok) {
        choice.confirm(enabled);
        setConfirming(false);
      }
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  };

  const cancelConfirmation = () => {
    if (!inFlight.current) setConfirming(false);
  };
  const changeTextModelAccess = (enabled: boolean) => {
    if (enabled) setConfirming(true);
    else void saveTextModelAccess(false);
  };

  return <>
    <SettingToggle
      label={t("settings.codexTextModels")}
      description={t("settings.codexTextModelsHint")}
      checked={choice.value}
      disabled={!available || saving || confirming}
      onChange={changeTextModelAccess}
    />
    {confirming ? <CodexTextModelsConfirmation
      saving={saving}
      onCancel={cancelConfirmation}
      onConfirm={() => void saveTextModelAccess(true)}
    /> : null}
  </>;
}

function CodexTextModelsConfirmation({ saving, onCancel, onConfirm }: {
  saving: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  const warningId = useId();
  const [deadline] = useState(() => performance.now() + CONFIRM_DELAY_MS);
  const [remaining, setRemaining] = useState(CONFIRM_DELAY_MS / 1_000);
  useEffect(() => {
    const timer = window.setInterval(() => {
      const seconds = Math.max(0, Math.ceil((deadline - performance.now()) / 1_000));
      setRemaining(seconds);
      if (seconds === 0) window.clearInterval(timer);
    }, 200);
    return () => window.clearInterval(timer);
  }, [deadline]);
  const confirm = () => {
    if (!saving && performance.now() >= deadline) onConfirm();
  };
  return <Dialog
    layer="top"
    className="codex-text-risk-dialog"
    title={t("settings.codexTextModelsConfirmTitle")}
    titleIcon={<TriangleAlert aria-hidden />}
    onClose={onCancel}
    footer={<>
      <Button variant="secondary" disabled={saving} onClick={onCancel}>{t("common.cancel")}</Button>
      <Button variant="danger" disabled={remaining > 0 || saving} busy={saving} aria-describedby={warningId} onClick={confirm}>
        {remaining > 0 ? t("settings.codexTextModelsConfirmWait", { seconds: remaining }) : t("settings.codexTextModelsConfirm")}
      </Button>
    </>}
  >
    <div id={warningId} className="codex-text-risk-copy">
      <p>{t("settings.codexTextModelsRiskAccess")}</p>
      <p>{t("settings.codexTextModelsRiskModels")}</p>
      <p>{t("settings.codexTextModelsRiskScope")}</p>
    </div>
  </Dialog>;
}
