import { useEffect, useState } from "react";

export function RotationNumberInput({ value, saving, emptyZero = false, onChange, ...props }: {
  value: number;
  saving: boolean;
  emptyZero?: boolean;
  onChange: (value: number) => void;
  "aria-label": string;
  "aria-valuetext"?: string | undefined;
  placeholder?: string;
  disabled: boolean;
  min: number;
  max: number;
}) {
  const text = emptyZero && value === 0 ? "" : String(value);
  const [draft, setDraft] = useState(text);
  useEffect(() => {
    if (!saving) setDraft(text);
  }, [saving, text]);
  return <input {...props} type="number" value={draft}
    onBlur={() => { if (!saving) setDraft(text); }}
    onChange={(event) => {
      setDraft(event.currentTarget.value);
      const numeric = emptyZero && event.currentTarget.value === "" ? 0 : event.currentTarget.valueAsNumber;
      if (Number.isSafeInteger(numeric)) onChange(numeric);
    }} />;
}
