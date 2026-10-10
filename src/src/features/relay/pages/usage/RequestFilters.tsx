import { useTranslation } from "react-i18next";
import { Search, X } from "lucide-react";
import { Button, IconButton, OptionMenu } from "../../components/Ui";
import type { UsageRow } from "./usageData";
import { formatErrorCategory } from "./usageReportFormat";

export function RequestFilters({
  rows,
  wireApi,
  onWireApiChange,
  transport,
  onTransportChange,
  errorQuery,
  onErrorChange,
  requestQuery,
  onRequestChange,
  onReset,
  onClose,
}: {
  rows: UsageRow[];
  wireApi: string;
  onWireApiChange: (value: string) => void;
  transport: string;
  onTransportChange: (value: string) => void;
  errorQuery: string;
  onErrorChange: (value: string) => void;
  requestQuery: string;
  onRequestChange: (value: string) => void;
  onReset: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const hasFilters = Boolean(wireApi || transport || errorQuery || requestQuery);
  const errorOptions = [
    { value: "", label: t("usage.anyErrorCategory") },
    ...Array.from(new Set([
      ...rows.flatMap((row) => row.errorCategory ? [row.errorCategory] : []),
      ...(errorQuery ? [errorQuery] : []),
    ])).sort().map((errorCategory) => ({ value: errorCategory, label: formatErrorCategory(errorCategory, t) })),
  ];
  const menuProps = {
    className: "filter-option-menu",
    showSelectionIndicator: false,
    fitContent: true,
    align: "start" as const,
  };
  return <section id="usage-request-filters" className="usage-filter-panel" aria-label={t("usage.moreFilters")}>
    <header>
      <h2>{t("usage.moreFilters")}</h2>
      <div>
        {hasFilters ? <Button variant="ghost" onClick={onReset}>{t("common.reset")}</Button> : null}
        <IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={onClose} />
      </div>
    </header>
    <div className="usage-filters">
      <div className="usage-filter-field">
        <span>{t("usage.protocol")}</span>
        <OptionMenu {...menuProps}
          label={t("usage.protocol")}
          value={wireApi}
          onChange={onWireApiChange}
          options={[
            { value: "", label: t("usage.anyProtocol") },
            { value: "responses", label: "Responses" },
            { value: "messages", label: "Messages" },
            { value: "chat_completions", label: "Chat Completions" },
            { value: "gemini", label: "Gemini" },
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
            { value: "http", label: t("usage.transports.http") },
            { value: "websocket", label: t("usage.transports.websocket") },
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
