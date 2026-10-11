import { Layers3, Minimize2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { ApplicationContextPolicy, CompactionPolicy, ContextWindowPolicy } from "../../api/types/runtime";
import { relayCommands } from "../../api/commands";
import { OptionMenu } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function ChatGPTContextSettings() {
  const { t } = useTranslation();
  const { mode, runtime, perform } = useRelayState();
  const saved: ApplicationContextPolicy = runtime?.gateway.chatgptContextPolicy
    ?? { mode: "configured", context: { mode: "client" }, compaction: { mode: "early" } };
  const [pending, setPending] = useState<ApplicationContextPolicy | null>(null);
  const [saving, setSaving] = useState(false);
  const selected = pending ?? saved;
  const context = selected.mode === "configured" ? selected.context : selected;
  const compaction: CompactionPolicy = selected.mode === "configured" ? selected.compaction
    : { mode: selected.mode === "client" ? "client" : "early" };
  useEffect(() => {
    if (pending && JSON.stringify(pending) === JSON.stringify(saved)) setPending(null);
  }, [pending, saved]);
  const save = async (window: ContextWindowPolicy, compact: CompactionPolicy) => {
    if (saving) return;
    const policy: ApplicationContextPolicy = { mode: "configured", context: window, compaction: compact };
    setPending(policy);
    setSaving(true);
    try {
      const ok = await perform("chatgpt-context", () => relayCommands.setChatgptContextPolicy(policy),
        "feedback.saved", { backgroundRefresh: true, uiLock: false });
      if (!ok) setPending(null);
    } finally { setSaving(false); }
  };
  if (mode !== "local") return null;
  return <><section className="gateway-setting-row codex-feature-control chatgpt-context-settings">
    <div className="codex-feature-heading">
      <span className="gateway-config-icon"><Layers3 aria-hidden /></span>
      <div><h2>{t("codex.contextTitle")}</h2></div>
    </div>
    <BudgetControl policy={context} disabled={saving || !runtime} label={t("codex.contextMode")}
      inputLabel={t("codex.contextTokens")} defaultTokens={272000}
      options={["client", "short", "extended", "custom"].map((id) => ({ value: id, label: t(`codex.contextModes.${id}`) }))}
      onChange={(policy) => void save(policy as ContextWindowPolicy, compaction)} />
  </section>
  <section className="gateway-setting-row codex-feature-control chatgpt-context-settings">
    <div className="codex-feature-heading">
      <span className="gateway-config-icon"><Minimize2 aria-hidden /></span>
      <div><h2>{t("codex.compactionTitle")}</h2></div>
    </div>
    <BudgetControl policy={compaction} disabled={saving || !runtime} label={t("codex.compactionMode")}
      inputLabel={t("codex.compactionTokens")} defaultTokens={240000}
      options={["client", "early", "custom"].map((id) => ({ value: id, label: t(`codex.compactionModes.${id}`) }))}
      onChange={(policy) => void save(context, policy as CompactionPolicy)} />
  </section></>;
}

function BudgetControl({ policy, disabled, label, inputLabel, defaultTokens, options, onChange }: {
  policy: ContextWindowPolicy | CompactionPolicy;
  disabled: boolean;
  label: string;
  inputLabel: string;
  defaultTokens: number;
  options: Array<{ value: string; label: string }>;
  onChange: (policy: ContextWindowPolicy | CompactionPolicy) => void;
}) {
  const { t } = useTranslation();
  const [custom, setCustom] = useState(policy.mode === "custom" ? String(policy.tokens) : String(defaultTokens));
  useEffect(() => {
    if (policy.mode === "custom") setCustom(String(policy.tokens));
  }, [policy.mode, policy.mode === "custom" ? policy.tokens : null]);
  const tokens = Number(custom);
  const valid = custom.trim() !== "" && Number.isSafeInteger(tokens) && tokens >= 1024 && tokens <= 2_000_000;
  return <div className="chatgpt-context-controls">
    <OptionMenu className="chatgpt-context-mode" label={label} value={policy.mode}
      fitContent disabled={disabled} options={options}
      onChange={(id) => onChange(id === "custom"
        ? { mode: "custom", tokens: valid ? tokens : defaultTokens }
        : { mode: id as "client" | "short" | "extended" | "early" })} />
    {policy.mode === "custom" ? <label className="chatgpt-context-budget">
      <input type="number" min={1024} max={2_000_000} step={1} value={custom}
        aria-label={inputLabel}
        aria-invalid={!valid || undefined} disabled={disabled} onChange={(event) => setCustom(event.target.value)}
        onBlur={() => { if (valid && policy.tokens !== tokens) onChange({ mode: "custom", tokens }); }}
        onKeyDown={(event) => { if (event.key === "Enter" && valid) event.currentTarget.blur(); }} />
      <span aria-hidden>{t("codex.contextUnit")}</span>
    </label> : null}
    {!valid && policy.mode === "custom" ? <small role="alert">{t("codex.contextInvalid")}</small> : null}
  </div>;
}
