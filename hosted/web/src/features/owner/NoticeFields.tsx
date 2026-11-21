import { useId } from "react";
import { useLocale } from "../../i18n/LocaleProvider";

export function NoticeFields({ title, message, disabled, onChange }: {
  title: string; message: string; disabled?: boolean; onChange: (fields: { title: string; message: string }) => void;
}) {
  const { t } = useLocale();
  const id = useId();
  return <>
    <label htmlFor={id + "-title"}>{t("ownerNotices.title")}</label>
    <input id={id + "-title"} dir="auto" autoFocus required maxLength={120} disabled={disabled}
      value={title} onChange={event => onChange({ title: event.target.value, message })} />
    <label htmlFor={id + "-message"}>{t("ownerNotices.message")}</label>
    <textarea id={id + "-message"} dir="auto" required maxLength={4000} rows={5} disabled={disabled}
      value={message} onChange={event => onChange({ title, message: event.target.value })} />
  </>;
}
