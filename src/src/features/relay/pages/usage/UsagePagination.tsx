import { useEffect, useId, useState, type FormEvent } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, IconButton } from "../../components/Ui";

type UsagePaginationProps = {
  page: number;
  totalPages: number;
  loading: boolean;
  onPageChange: (page: number) => void;
};

export function UsagePagination({ page, totalPages, loading, onPageChange }: UsagePaginationProps) {
  const { t } = useTranslation();
  const inputId = useId();
  const errorId = useId();
  const [draft, setDraft] = useState(String(page));
  const [invalid, setInvalid] = useState(false);

  useEffect(() => {
    setDraft(String(page));
    setInvalid(false);
  }, [page, totalPages]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (loading) return;
    const value = draft.trim();
    const next = Number(value);
    if (!/^[1-9]\d*$/.test(value) || !Number.isSafeInteger(next) || next > totalPages) {
      setInvalid(true);
      return;
    }
    if (next !== page) onPageChange(next);
  };

  return <nav className="usage-pagination" aria-label={t("usage.pagination")}>
    <IconButton label={t("common.back")} icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || loading} onClick={() => onPageChange(page - 1)} />
    <form className="usage-pagination-form" onSubmit={submit} noValidate>
      <label htmlFor={inputId}>{t("usage.pageNumber")}</label>
      <input id={inputId} type="text" inputMode="numeric" autoComplete="off" value={draft} disabled={loading} aria-invalid={invalid || undefined} aria-describedby={invalid ? errorId : undefined} onChange={(event) => { setDraft(event.target.value); setInvalid(false); }} />
      <span>{t("usage.ofPages", { total: totalPages })}</span>
      <Button type="submit" disabled={loading || draft.trim() === String(page)}>{t("usage.goToPage")}</Button>
    </form>
    <IconButton label={t("common.continue")} icon={<ChevronRight aria-hidden />} disabled={page >= totalPages || loading} onClick={() => onPageChange(page + 1)} />
    {invalid ? <span id={errorId} role="alert" className="usage-pagination-error">{t("usage.invalidPage", { total: totalPages })}</span> : null}
  </nav>;
}
