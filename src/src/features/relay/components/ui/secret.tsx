import { useId, useState } from "react";
import type { ReactNode } from "react";
import { CheckCircle2, Copy, Eye, EyeOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { Button, IconButton } from "./buttons";

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
  labelAction,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  labelAction?: ReactNode;
}) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const inputId = useId();
  return <div className="relay-field">
    {labelAction
      ? <div className="relay-field-label-row">
        <label htmlFor={inputId}>{label}</label>
        {labelAction}
      </div>
      : <label htmlFor={inputId}>{label}</label>}
    <div className="secret-field">
      <input id={inputId} type={visible ? "text" : "password"} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} autoComplete="off" spellCheck={false} />
      <IconButton label={visible ? t("common.hide") : t("common.reveal")} icon={visible ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setVisible((current) => !current)} type="button" />
    </div>
  </div>;
}

export async function copyText(value: string) {
  await navigator.clipboard.writeText(value);
}

export function CopyButton({ value, label, children }: { value: string; label: string; children?: ReactNode }) {
  const { t } = useTranslation();
  const [copied, showCopied] = useTransientFlag(1_500);
  const accessibleLabel = copied ? `${label}: ${t("feedback.copied")}` : label;
  const icon = copied ? <CheckCircle2 aria-hidden /> : <Copy aria-hidden />;
  const onClick = async () => { await copyText(value); showCopied(); };
  return children !== undefined
    ? <Button aria-label={accessibleLabel} icon={icon} onClick={onClick}>{children}</Button>
    : <IconButton label={accessibleLabel} icon={icon} onClick={onClick} />;
}
