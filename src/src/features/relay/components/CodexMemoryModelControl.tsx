import { useEffect, useState } from "react";
import { Brain, ScanText } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../api/commands";
import type { CodexMemoryStage } from "../api/types";
import { useRelayState } from "../state/RelayStateProvider";
import { usePendingFlag } from "../state/usePendingFlag";
import { Button, OptionMenu, ToggleSwitch } from "./Ui";

export function CodexMemoryModelControl({ stage }: { stage: CodexMemoryStage }) {
  const { t } = useTranslation();
  const { runtime, perform } = useRelayState();
  const [model, setModel] = useState<string | null>();
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<"load" | "save" | null>(null);
  const [reload, setReload] = useState(0);
  const extraction = stage === "extraction";
  const title = t(extraction ? "codex.memoryExtractionTitle" : "codex.memoryConsolidationTitle");
  const modelLabel = t(extraction ? "codex.memoryExtractionModelTitle" : "codex.memoryModelTitle");
  const enabled = usePendingFlag((extraction ? runtime?.gateway.codexMemoryExtractionEnabled : runtime?.gateway.codexMemoryConsolidationEnabled) ?? true);
  const Icon = extraction ? ScanText : Brain;
  useEffect(() => {
    let active = true;
    void relayCommands.getCodexMemoryModel(stage).then((value) => {
      if (active) { setModel(value); setFailure(null); }
    }).catch(() => { if (active) setFailure("load"); });
    return () => { active = false; };
  }, [reload, stage]);

  const models = (runtime?.gateway.models ?? []).filter((candidate) => candidate.enabled && candidate.codexVisible);
  const options = [
    { value: "", label: t("codex.memoryModelDefault") },
    ...models.map((candidate) => ({ value: candidate.id, label: candidate.id })),
  ];
  if (model && !models.some((candidate) => candidate.id === model)) {
    options.push({ value: model, label: model });
  }
  const save = async (next: string) => {
    if (model === undefined || saving || (next || null) === model) return;
    setSaving(true);
    setFailure(null);
    try {
      const ok = await perform(`codex-memory-model-${stage}`, async () => {
        setModel(await relayCommands.setCodexMemoryModel(stage, next || null, model));
      }, "feedback.saved", { skipRefresh: true, uiLock: false });
      if (!ok) setFailure("save");
    } finally { setSaving(false); }
  };

  return <section className="gateway-setting-row codex-memory-model-control" data-memory-stage={stage}>
    <header className="codex-feature-heading">
      <span className="gateway-config-icon"><Icon aria-hidden /></span>
      <div><h2>{title}</h2><p>{t(extraction ? "codex.memoryExtractionHint" : "codex.memoryConsolidationHint")}</p></div>
    </header>
    <div className="codex-memory-model-options">
      <div className="codex-memory-model-actions">
        <OptionMenu className="field-option-menu codex-memory-model-menu" label={modelLabel} value={model ?? ""}
          options={options} onChange={(next) => void save(next)} disabled={!runtime || model === undefined || saving || failure === "load"} />
        <ToggleSwitch label={t(extraction ? "codex.memoryExtractionToggle" : "codex.memoryConsolidationToggle")}
          checked={enabled.checked} disabled={!runtime}
          onChange={(next) => enabled.select(next, () => perform(`codex-memory-enabled-${stage}`,
            () => relayCommands.setCodexMemoryEnabled(stage, next), "feedback.saved", { backgroundRefresh: true, uiLock: false }))} />
      </div>
      {failure ? <div className="application-launch-path-feedback" role="alert">
        <span>{t(failure === "load" ? "codex.memoryModelLoadFailed" : "codex.memoryModelSaveFailed")}</span>
        <Button disabled={saving} onClick={() => setReload((value) => value + 1)}>{t("common.retry")}</Button>
      </div> : null}
    </div>
  </section>;
}
