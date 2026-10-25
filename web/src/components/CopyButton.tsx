import { useState } from "react";
import { Copy } from "lucide-react";

export function CopyButton({ text, label, compact = false }: { text: string; label: string; compact?: boolean }) {
  const [status, setStatus] = useState<"idle" | "copied" | "manual">("idle");
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setStatus("copied");
    } catch { setStatus("manual"); }
  }
  return <span className="copy-control">
    <button type="button" className={compact ? "icon-button" : "button"} aria-label={label} onClick={() => { void copy(); }}>
      <Copy aria-hidden="true" />{!compact && label}
    </button>
    {status !== "idle" && <span className="form-note" role="status">{status === "copied" ? "Скопировано." : "Копирование недоступно. Выделите текст вручную."}</span>}
    {status === "manual" && <label className="copy-fallback">{label}<textarea readOnly value={text} onFocus={event => event.target.select()} /></label>}
  </span>;
}
