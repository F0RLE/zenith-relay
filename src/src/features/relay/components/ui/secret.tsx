import { useId, useState } from "react";
import type { FocusEventHandler, ReactNode } from "react";
import { CheckCircle2, Copy, Eye, EyeOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { Button, IconButton } from "./buttons";
import { copyText } from "./clipboard";

export { copyText } from "./clipboard";

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
  labelAction,
  disabled = false,
  onBlur,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  labelAction?: ReactNode;
  disabled?: boolean;
  onBlur?: FocusEventHandler<HTMLInputElement>;
}) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const [copied, showCopied, clearCopied] = useTransientFlag(1_500);
  const [copyFailed, setCopyFailed] = useState(false);
  const inputId = useId();
  const copy = async (text: string) => {
    try {
      await copyText(text);
      setCopyFailed(false);
      showCopied();
    } catch {
      clearCopied();
      setCopyFailed(true);
    }
  };
  const copyLabel = `${t("common.copy")}: ${label}`;
  return <div className="relay-field">
    {labelAction
      ? <div className="relay-field-label-row">
        <label htmlFor={inputId}>{label}</label>
        {labelAction}
      </div>
      : <label htmlFor={inputId}>{label}</label>}
    <div className="secret-field">
      <input
        id={inputId}
        type={visible ? "text" : "password"}
        value={value}
        onChange={(event) => { clearCopied(); setCopyFailed(false); onChange(event.target.value); }}
        onKeyDown={(event) => {
          if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey || (event.code !== "KeyC" && event.key.toLowerCase() !== "c")) return;
          event.preventDefault();
          event.stopPropagation();
          const input = event.currentTarget;
          const start = input.selectionStart ?? 0;
          const end = input.selectionEnd ?? start;
          if (end > start) void copy(input.value.slice(start, end));
        }}
        onBlur={onBlur}
        disabled={disabled}
        placeholder={placeholder}
        autoComplete="off"
        autoCapitalize="none"
        spellCheck={false}
      />
      <div className="secret-field-actions">
        <IconButton
          label={copyFailed ? `${copyLabel}: ${t("common.copyFailed")}` : copied ? `${copyLabel}: ${t("feedback.copied")}` : copyLabel}
          title={copyFailed ? t("common.copyFailed") : copied ? t("feedback.copied") : t("common.copy")}
          icon={copied ? <CheckCircle2 aria-hidden /> : <Copy aria-hidden />}
          onClick={() => void copy(value)}
          onPointerDown={(event) => event.preventDefault()}
          disabled={disabled || !value}
          data-copied={copied}
          aria-controls={inputId}
          aria-live="polite"
        />
        <IconButton
          label={`${visible ? t("common.hide") : t("common.reveal")}: ${label}`}
          title={visible ? t("common.hide") : t("common.reveal")}
          icon={visible ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          onClick={() => setVisible((previousVisibility) => !previousVisibility)}
          disabled={disabled}
          aria-controls={inputId}
          aria-pressed={visible}
        />
      </div>
    </div>
  </div>;
}

export function CopyButton({ value: copyValue, label, children }: { value: string; label: string; children?: ReactNode }) {
  const { t } = useTranslation();
  const [copied, showCopied, clearCopied] = useTransientFlag(1_500);
  const [failed, setFailed] = useState(false);
  const accessibleLabel = failed ? `${label}: ${t("common.copyFailed")}` : copied ? `${label}: ${t("feedback.copied")}` : label;
  const icon = copied ? <CheckCircle2 aria-hidden /> : <Copy aria-hidden />;
  const onClick = async () => {
    try { await copyText(copyValue); setFailed(false); showCopied(); }
    catch { clearCopied(); setFailed(true); }
  };
  return children !== undefined
    ? <Button aria-label={accessibleLabel} icon={icon} onClick={onClick}>{children}</Button>
    : <IconButton label={accessibleLabel} icon={icon} onClick={onClick} />;
}
