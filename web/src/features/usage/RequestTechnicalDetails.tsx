import { Braces, ChevronDown } from "../../components/Icons";
import { CopyButton } from "../../components/CopyButton";
import { accountingLabels } from "./model";
import type { RequestRecord } from "./model";
import { useLocale } from "../../i18n/LocaleProvider";

// Render only the customer's Hosted projection, never provider accounts or local runtime state.
export function RequestTechnicalDetails({ record }: { record: RequestRecord }) {
  const { t, localize } = useLocale();
  return <details className="request-technical-details">
    <summary><span className="request-disclosure-icon"><Braces aria-hidden="true" /></span><span>{t("request.technicalDetails")}</span><ChevronDown className="request-disclosure-chevron" aria-hidden="true" /></summary>
    <div className="request-technical-body">
      <dl className="request-technical-grid">
        <div className="request-id"><dt>{t("request.requestId")}</dt><dd><code>{record.id}</code><CopyButton text={record.id} label={t("request.copyRequestId")} compact /></dd></div>
        <div><dt>{t("common.model")}</dt><dd><code>{record.model.id}</code></dd></div>
        <div><dt>{t("common.protocol")}</dt><dd>{record.protocol}</dd></div>
        <div><dt>{t("request.appTransport")}</dt><dd>{record.transport}</dd></div>
        <div><dt>{t("request.serviceTier")}</dt><dd>{localize(record.tier)}</dd></div>
        <div><dt>{t("request.reasoning")}</dt><dd>{localize(record.model.reasoningEffort)}</dd></div>
        {record.execution === "failed" && record.response.httpStatus !== null && <div><dt>{t("request.httpStatus")}</dt><dd>{record.response.httpStatus}</dd></div>}
        {record.execution === "failed" && record.response.errorCode && <div><dt>{t("common.errorCode")}</dt><dd><code>{record.response.errorCode}</code></dd></div>}
        {record.accounting !== "posted" && <div><dt>{t("usage.accounting")}</dt><dd>{t(accountingLabels[record.accounting])}</dd></div>}
      </dl>
    </div>
  </details>;
}
