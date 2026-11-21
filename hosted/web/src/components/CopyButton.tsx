import { useEffect, useState } from "react";
import { Check, Copy } from "./Icons";
import { useLocale } from "../i18n/LocaleProvider";

export function CopyButton({ text, label, compact = false }: { text: string; label: string; compact?: boolean }) {
  const { t } = useLocale();
  const [status, setStatus] = useState<"idle" | "copied" | "manual">("idle");
  useEffect(() => {
    if (!compact || status !== "copied") return;
    const timeout = window.setTimeout(() => setStatus("idle"), 1800);
    return () => window.clearTimeout(timeout);
  }, [compact, status]);
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setStatus("copied");
    } catch { setStatus("manual"); }
  }
  return <span className="copy-control">
    <button type="button" className={compact ? "icon-button" : "button"} aria-label={label} data-relay-tooltip={compact ? label : undefined} onClick={() => { void copy(); }}>
      {compact && status === "copied" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}{!compact && label}
    </button>
    {status !== "idle" && <span className={compact && status === "copied" ? "sr-only" : "form-note"} role="status">{t(status === "copied" ? "common.copied" : "common.copyUnavailable")}</span>}
    {status === "manual" && <label className="copy-fallback">{label}<textarea readOnly value={text} onFocus={event => event.target.select()} /></label>}
  </span>;
}
