import { useTranslation } from "react-i18next";
import { CopyButton } from "../../components/Ui";
import { formatFullNumber } from "../../usageTotals";
import { durationLabel, requestCacheStorage } from "./requestCacheStorage";
import { RequestDisclosure } from "./RequestDisclosure";
import { hasRequestParameters, RequestParameters } from "./RequestParameters";
import type { UsageRow } from "./usageData";
import { differentAppliedTier, formatErrorOrigin, formatModelId, formatTransport, requestStatusKind } from "./usageReportFormat";

function connectionChoice(routing: UsageRow["routing"]) {
  if (!routing) return null;
  if (routing.reason === "response_affinity") return "responseOwner";
  if (routing.cacheContext?.baseline === "completed_request" && routing.cacheContext.candidateChanged === true) {
    if (routing.cacheAffinity?.reason === "owner_unavailable") {
      if (routing.cacheAffinity.ownerBlockReason === "rate_limited") return "rateLimit";
      if (routing.cacheAffinity.ownerBlockReason === "quota_exhausted") return "quota";
    }
    return "changed";
  }
  if (routing.cacheAffinity?.selection === "warm_alternative") return "usedBefore";
  if (routing.reason === "prompt_cache_affinity" || routing.cacheAffinity?.selection === "preferred") return "chatOwner";
  return null;
}

export function RequestReceiptDetails({ row }: { row: UsageRow }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const fields: { key: string; label: string; value: string; copy?: string | undefined }[] = [];
  const add = (label: string, value: string | null | undefined, copy?: string) => {
    if (value) fields.push({ key: label, label: t(label), value, copy });
  };
  const choice = connectionChoice(row.routing);
  if (choice) add("usage.connectionChoice", t(`usage.connectionChoices.${choice}`));
  const upstream = row.routing?.transport?.upstream;
  if (upstream && upstream !== row.transport) add("usage.providerTransport", formatTransport(upstream, t));
  add("usage.upstreamTier", differentAppliedTier(row, t));

  const { requestedModel, routedModel } = row;
  const alias = requestedModel && formatModelId(requestedModel, routedModel) !== requestedModel;
  const declaredModel = row.routing?.upstreamResponseModel;
  if (alias || (routedModel && requestedModel && requestedModel !== routedModel)) {
    add("usage.requestedModel", requestedModel, "usage.copyModelId");
    add("usage.routedModel", routedModel);
  }
  if (declaredModel !== (routedModel ?? requestedModel)) add("usage.upstreamResponseModel", declaredModel);

  const { mode, retention, reported, writes } = requestCacheStorage(row.toolUse?.providerUsage, row.cacheWriteTtl);
  if (mode) add("usage.cacheReport.mode", t(`usage.cacheReport.${mode}`));
  if (retention === "in_memory") add("usage.cacheReport.storage", t("usage.cacheReport.inMemory"));
  else if (retention) add("usage.cacheReport.defaultRetention", durationLabel(retention, locale));
  if (reported.length) add("usage.cacheReport.retention", reported.map((window) => durationLabel(window, locale)).join(" / "));
  for (const { key, window, count } of writes) {
    fields.push({ key: `write:${key}`, label: t("usage.cacheReport.writtenFor", { duration: durationLabel(window, locale) }),
      value: t("usage.cacheReport.tokens", { value: formatFullNumber(count, locale) }) });
  }

  if (requestStatusKind(row) === "error") {
    if (row.errorOrigin) add("usage.errorOrigin", formatErrorOrigin(row.errorOrigin, t));
    const error = row.upstreamError;
    if (error?.httpStatus != null && error.httpStatus !== row.httpStatus) add("usage.upstreamHttpStatus", String(error.httpStatus));
    add("usage.upstreamErrorCode", error?.code);
    if (error?.errorType !== error?.code) add("usage.upstreamErrorType", error?.errorType);
  }
  add("usage.localRequestId", row.requestId, "usage.copyRequestId");
  add("usage.upstreamRequestId", row.upstreamError?.requestId, "usage.copyUpstreamRequestId");
  if (!fields.length && !hasRequestParameters(row)) return null;
  return <RequestDisclosure title={t("usage.receiptDetails")} className="request-technical-details">
    {() => <><RequestParameters row={row} />{fields.length > 0 ? <dl className="request-details-list request-id-list">
      {fields.map(({ key, label, value, copy }) => <div key={key}>
        <dt>{label}</dt>
        <dd className={copy ? "request-details-copy-value" : undefined}>
          {copy ? <><code>{value}</code><CopyButton value={value} label={t(copy)} /></> : value}
        </dd>
      </div>)}
    </dl> : null}</>}
  </RequestDisclosure>;
}
