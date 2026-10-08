import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/Ui";

export function matchesQuery(query: string, ...searchValues: Array<string | string[] | null | undefined>) {
  const normalized = query.trim().toLocaleLowerCase();
  return !normalized || searchValues
    .flatMap((searchValue) => Array.isArray(searchValue) ? searchValue : searchValue ?? [])
    .some((searchValue) => searchValue.toLocaleLowerCase().includes(normalized));
}

export function NoResults() {
  const { t } = useTranslation();
  return <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
}
