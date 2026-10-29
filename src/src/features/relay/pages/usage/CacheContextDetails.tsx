import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import type { CacheContextDiagnostics, CacheContextSection, CacheHistoryDiagnostics } from "../../api/types";
import { formatFullNumber } from "../../usageTotals";
import { formatDurationMs } from "./usageReportFormat";

export function CacheContextDetails({ diagnostics }: { diagnostics: CacheContextDiagnostics | null | undefined }) {
  const { t, i18n } = useTranslation();
  if (!diagnostics) return null;
  const field = (label: string, value: ReactNode) => <div key={label}><dt>{t(label)}</dt><dd>{value}</dd></div>;
  // Structural differences do not prove why a provider missed its prompt cache.
  const compared = diagnostics.baseline === "completed_request";
  const sections = (changes: CacheContextSection[]) => changes.length
    ? changes.map((section) => t(`usage.cacheContext.sections.${section}`)).join(", ")
    : t("usage.cacheContext.noChanges");
  return <div className="request-cache-context">
    <dl className="request-details-list">
      {diagnostics.provider ? <div>
        <dt>{t("usage.cacheContext.provider")}</dt>
        <dd>{t(`usage.cacheContext.providerResults.${diagnostics.provider.type}`)}
          {diagnostics.provider.type === "cache_miss" ? <> · {t(`usage.cacheContext.providerReasons.${diagnostics.provider.reason}`)}</> : null}
        </dd>
      </div> : null}
      {field("usage.cacheContext.baseline", t(`usage.cacheContext.baselines.${diagnostics.baseline}`))}
      {field("usage.cacheContext.scope", t(`usage.cacheContext.scopes.${diagnostics.scope}`))}
      {compared && diagnostics.previousCompletedAgeMs != null
        ? field("usage.cacheContext.previousAge", formatDurationMs(diagnostics.previousCompletedAgeMs, i18n.resolvedLanguage ?? i18n.language, t)) : null}
      {compared && diagnostics.candidateChanged != null
        ? field("usage.cacheContext.candidate", t(diagnostics.candidateChanged ? "usage.cacheContext.candidateChanged" : "usage.cacheContext.candidateUnchanged")) : null}
      {compared ? <>
        {field("usage.cacheContext.clientChanges", sections(diagnostics.clientChanges))}
        {field("usage.cacheContext.upstreamChanges", sections(diagnostics.upstreamChanges))}
      </> : null}
      {diagnostics.relayHistory.comparison !== "not_compared"
        ? field("usage.cacheContext.relayChanges", sections(diagnostics.relayChanges)) : null}
      {field("usage.cacheContext.clientHistory", <History diagnostics={diagnostics.clientHistory} />)}
      {field("usage.cacheContext.upstreamHistory", <History diagnostics={diagnostics.upstreamHistory} />)}
      {field("usage.cacheContext.relayHistory", <History diagnostics={diagnostics.relayHistory} />)}
    </dl>
  </div>;
}

function History({ diagnostics }: { diagnostics: CacheHistoryDiagnostics }) {
  const { t, i18n } = useTranslation();
  const number = (value: number) => formatFullNumber(value, i18n.language);
  return <>
    <span>{t(`usage.cacheContext.histories.${diagnostics.comparison}`)}</span>
    {diagnostics.inputItems != null ? <small>{t("usage.cacheContext.items", { value: number(diagnostics.inputItems) })}</small> : null}
    {diagnostics.inputBytes != null ? <small>{t("usage.cacheContext.jsonBytes", { value: number(diagnostics.inputBytes) })}</small> : null}
    {diagnostics.sharedPrefixItems != null ? <small>{t("usage.cacheContext.sharedItems", { value: number(diagnostics.sharedPrefixItems) })}</small> : null}
    {diagnostics.firstChangedItemKind ? <small>{t("usage.cacheContext.firstChanged", {
      kind: t(`usage.cacheContext.kinds.${diagnostics.firstChangedItemKind}`),
    })}</small> : null}
  </>;
}
