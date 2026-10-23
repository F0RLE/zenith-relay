import { useTranslation } from "react-i18next";
import { Search } from "lucide-react";
import { OptionMenu } from "../../components/Ui";
import type { UsageFilterOptions } from "../../api/types";
import { formatErrorCategory, formatTransport, formatWireApi } from "./usageReportFormat";

function availableValues(values: string[] | undefined, selected: string): string[] {
  return Array.from(new Set([...(values ?? []), ...(selected ? [selected] : [])])).sort();
}

export function RequestFilters({
  filterOptions,
  wireApi,
  onWireApiChange,
  transport,
  onTransportChange,
  errorQuery,
  onErrorChange,
  requestQuery,
  onRequestChange,
}: {
  filterOptions: Partial<UsageFilterOptions> | undefined;
  wireApi: string;
  onWireApiChange: (value: string) => void;
  transport: string;
  onTransportChange: (value: string) => void;
  errorQuery: string;
  onErrorChange: (value: string) => void;
  requestQuery: string;
  onRequestChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  const errorOptions = [
    { value: "", label: t("usage.anyErrorCategory") },
    ...availableValues(filterOptions?.errorCategories, errorQuery)
      .map((errorCategory) => ({ value: errorCategory, label: formatErrorCategory(errorCategory, t) })),
  ];
  const menuProps = {
    className: "filter-option-menu",
    showSelectionIndicator: false,
    align: "start" as const,
  };
  return <section id="usage-request-filters" className="usage-filter-panel" aria-label={t("usage.moreFilters")}>
    <div className="usage-filters">
      <div className="usage-filter-field">
        <span>{t("usage.protocol")}</span>
        <OptionMenu {...menuProps}
          label={t("usage.protocol")}
          value={wireApi}
          onChange={onWireApiChange}
          options={[
            { value: "", label: t("usage.anyProtocol") },
            ...availableValues(filterOptions?.wireApis, wireApi)
              .map((value) => ({ value, label: formatWireApi(value, t) })),
          ]}
        />
      </div>
      <div className="usage-filter-field">
        <span>{t("usage.transport")}</span>
        <OptionMenu {...menuProps}
          label={t("usage.transport")}
          value={transport}
          onChange={onTransportChange}
          options={[
            { value: "", label: t("usage.anyTransport") },
            ...availableValues(filterOptions?.transports, transport)
              .map((value) => ({ value, label: formatTransport(value, t) })),
          ]}
        />
      </div>
      <div className="usage-filter-field">
        <span>{t("usage.errorCategory")}</span>
        <OptionMenu {...menuProps} label={t("usage.errorCategory")} value={errorQuery} onChange={onErrorChange} options={errorOptions} />
      </div>
      <label className="usage-filter-field">
        <span>{t("usage.requestId")}</span>
        <span className="usage-request-search">
          <Search aria-hidden />
          <input value={requestQuery} onChange={(event) => onRequestChange(event.target.value)} placeholder={t("usage.requestId")} />
        </span>
      </label>
    </div>
  </section>;
}
