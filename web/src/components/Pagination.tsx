import { ChevronLeft, ChevronRight } from "./Icons";
import { OptionMenu } from "./OptionMenu";
import { useLocale } from "../i18n/LocaleProvider";
import "./pagination.css";

export function Pagination({ label, count, page, size, sizes, onPage, onSize, suffix = "" }: {
  label: string; count: number; page: number; size: number; sizes: number[];
  onPage: (page: number) => void; onSize: (size: number) => void; suffix?: string;
}) {
  const { t, locale } = useLocale();
  const pages = Math.max(1, Math.ceil(count / size));
  const current = Math.max(1, Math.min(page, pages));
  return <nav className="pagination" aria-label={label}>
    <span role="status">{count ? t("pagination.range", { start: ((current - 1) * size + 1).toLocaleString(locale), end: Math.min(current * size, count).toLocaleString(locale), count: count.toLocaleString(locale) }) : t("pagination.empty")}{suffix}</span>
    {count > 0 && <div><label><span>{t("pagination.rows")}</span><OptionMenu label={t("pagination.rows")} value={String(size)} onChange={value => onSize(Number(value))} options={sizes.map(value => ({ value: String(value), label: value.toLocaleString(locale) }))} /></label>
      <button type="button" className="icon-button" aria-label={t("pagination.previousPage")} disabled={current === 1} onClick={() => onPage(current - 1)}><ChevronLeft aria-hidden="true" /></button>
      <span>{current.toLocaleString(locale)} / {pages.toLocaleString(locale)}</span>
      <button type="button" className="icon-button" aria-label={t("pagination.nextPage")} disabled={current === pages} onClick={() => onPage(current + 1)}><ChevronRight aria-hidden="true" /></button>
    </div>}
  </nav>;
}
