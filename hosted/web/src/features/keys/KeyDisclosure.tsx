import { useEffect, useRef } from "react";
import { CopyButton } from "../../components/CopyButton";
import { useLocale } from "../../i18n/LocaleProvider";

export function KeyDisclosure({ secret, onDone }: { secret: string; onDone: () => void }) {
  const { t } = useLocale();
  const panel = useRef<HTMLElement>(null);
  useEffect(() => { panel.current?.querySelector("button")?.focus(); }, [secret]);
  return <section className="key-reveal" aria-label={t("keys.keyCreated")} ref={panel}>
    <h2>{t("keys.keyCreated")}</h2><p>{t("keys.secretWarning")}</p>
    <code>{secret}</code>
    <div className="key-reveal-actions"><CopyButton text={secret} label={t("keys.copyKey")} />
      <button className="button" type="button" onClick={onDone}>{t("common.done")}</button></div>
  </section>;
}
