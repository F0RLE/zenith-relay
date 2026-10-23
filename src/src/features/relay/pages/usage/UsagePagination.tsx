import { Fragment, useEffect, useId, useRef, useState, type FormEvent } from "react";
import { Check, ChevronLeft, ChevronRight, X } from "lucide-react";
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
  const [editing, setEditing] = useState(false);
  const navigation = useRef<HTMLElement>(null);
  const returnFocus = useRef(false);

  const visiblePages = totalPages <= 5
    ? Array.from({ length: totalPages }, (_, index) => index + 1)
    : page <= 3 ? [1, 2, 3, totalPages]
      : page >= totalPages - 2 ? [1, totalPages - 2, totalPages - 1, totalPages]
        : [1, page, totalPages];

  const closeEditor = () => {
    setDraft(String(page));
    setInvalid(false);
    returnFocus.current = true;
    setEditing(false);
  };

  useEffect(() => {
    if (!editing && returnFocus.current) {
      navigation.current?.querySelector<HTMLButtonElement>('[aria-current="page"]')?.focus();
      returnFocus.current = false;
    }
  }, [editing]);

  useEffect(() => {
    setDraft(String(page));
    setInvalid(false);
    setEditing(false);
  }, [page, totalPages]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (loading) return;
    const trimmedPageDraft = draft.trim();
    const nextPage = Number(trimmedPageDraft);
    if (!/^[1-9]\d*$/.test(trimmedPageDraft) || !Number.isSafeInteger(nextPage) || nextPage > totalPages) {
      setInvalid(true);
      return;
    }
    closeEditor();
    if (nextPage !== page) onPageChange(nextPage);
  };

  return <nav ref={navigation} className="usage-pagination" aria-label={t("usage.pagination")}>
    <div className="usage-pagination-controls">
      {editing ? <form className="usage-pagination-form" onSubmit={submit} onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); closeEditor(); } }} noValidate>
        <label className="sr-only" htmlFor={inputId}>{t("usage.pageNumber")}</label>
        <input id={inputId} type="text" inputMode="numeric" autoComplete="off" autoFocus value={draft} disabled={loading} aria-invalid={invalid || undefined} aria-describedby={invalid ? errorId : undefined} onFocus={(event) => event.target.select()} onChange={(event) => { setDraft(event.target.value); setInvalid(false); }} />
        <span>{t("usage.ofPages", { total: totalPages })}</span>
        <IconButton type="submit" label={t("usage.goToPage")} icon={<Check aria-hidden />} disabled={loading} />
        <IconButton label={t("common.cancel")} icon={<X aria-hidden />} onClick={closeEditor} />
      </form> : <>
        <IconButton label={t("common.back")} icon={<ChevronLeft aria-hidden />} disabled={page <= 1 || loading} onClick={() => onPageChange(page - 1)} />
        {visiblePages.map((pageNumber, index) => <Fragment key={pageNumber}>
          {index > 0 && pageNumber - visiblePages[index - 1]! > 1 ? <Button className="usage-pagination-gap" variant="ghost" aria-label={t("usage.jumpToPage")} title={t("usage.jumpToPage")} disabled={loading} onClick={() => setEditing(true)}>…</Button> : null}
          <Button className="usage-pagination-page" variant="ghost" aria-label={t("usage.pageLabel", { page: pageNumber })} aria-current={pageNumber === page ? "page" : undefined} title={pageNumber === page ? t("usage.jumpToPage") : undefined} disabled={loading} onClick={() => pageNumber === page ? setEditing(true) : onPageChange(pageNumber)}>{pageNumber}</Button>
        </Fragment>)}
        <IconButton label={t("common.continue")} icon={<ChevronRight aria-hidden />} disabled={page >= totalPages || loading} onClick={() => onPageChange(page + 1)} />
      </>}
    </div>
    {invalid ? <span id={errorId} role="alert" className="usage-pagination-error">{t("usage.invalidPage", { total: totalPages })}</span> : null}
  </nav>;
}
