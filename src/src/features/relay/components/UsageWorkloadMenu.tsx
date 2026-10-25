import { useTranslation } from "react-i18next";
import type { UsageQuery } from "../api/types";
import { OptionMenu } from "./Ui";

export type UsageWorkloadSelection = NonNullable<UsageQuery["workload"]> | "";

/** Keep the full, honest total as the default. Scopes never change accounting. */
export function UsageWorkloadMenu({ value, onChange }: {
  value: UsageWorkloadSelection;
  onChange: (value: UsageWorkloadSelection) => void;
}) {
  const { t } = useTranslation();
  return <OptionMenu
    className="usage-workload-menu"
    label={t("usage.workload.label")}
    value={value}
    onChange={(next) => onChange(next as UsageWorkloadSelection)}
    showSelectionIndicator={false}
    options={[
      { value: "", label: t("usage.workload.all") },
      { value: "conversation", label: t("usage.workload.conversation") },
      { value: "memory", label: t("usage.workload.memory") },
      { value: "background", label: t("usage.workload.background") },
    ]}
  />;
}
