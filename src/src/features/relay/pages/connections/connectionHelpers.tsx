import { useTranslation } from "react-i18next";
import { EmptyState } from "../../components/Ui";

export function matchesQuery(query: string, ...values: Array<string | string[] | null | undefined>) {
  const normalized = query.trim().toLocaleLowerCase();
  return !normalized || values
    .flatMap((value) => Array.isArray(value) ? value : value ?? [])
    .some((value) => value.toLocaleLowerCase().includes(normalized));
}

export function NoResults() {
  const { t } = useTranslation();
  return <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
}
