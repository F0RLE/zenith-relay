import { useTranslation } from "react-i18next";
import type { CacheContextDiagnostics, CacheContextSection, CacheHistoryDiagnostics } from "../../api/types";
import { formatFullNumber } from "../../usageTotals";
import { formatDurationMs } from "./usageReportFormat";

export function CacheContextDetails({ diagnostics }: { diagnostics: CacheContextDiagnostics | null | undefined }) {
  const { t, i18n } = useTranslation();
  if (!diagnostics) return null;
  const compared = diagnostics.baseline === "completed_request";
  const sections = (changes: CacheContextSection[]) => changes.length
    ? changes.map((section) => t(`usage.cacheContext.sections.${section}`)).join(", ")
    : t("usage.cacheContext.noChanges");
  return <section className="request-details-section request-cache-context" aria-label={t("usage.cacheContext.title")}>
    <h3>{t("usage.cacheContext.title")}</h3>
    <p className="form-note">{t("usage.cacheContext.hint")}</p>
    <dl className="request-details-list">
      <div><dt>{t("usage.cacheContext.baseline")}</dt><dd>{t(`usage.cacheContext.baselines.${diagnostics.baseline}`)}</dd></div>
      <div><dt>{t("usage.cacheContext.scope")}</dt><dd>{t(`usage.cacheContext.scopes.${diagnostics.scope}`)}</dd></div>
      {compared && diagnostics.previousCompletedAgeMs != null ? <div>
        <dt>{t("usage.cacheContext.previousAge")}</dt>
        <dd>{formatDurationMs(diagnostics.previousCompletedAgeMs, i18n.resolvedLanguage ?? i18n.language, t)}</dd>
      </div> : null}
      {compared && diagnostics.candidateChanged != null ? <div>
        <dt>{t("usage.cacheContext.candidate")}</dt>
        <dd>{t(diagnostics.candidateChanged ? "usage.cacheContext.candidateChanged" : "usage.cacheContext.candidateUnchanged")}</dd>
      </div> : null}
      {compared ? <>
        <div><dt>{t("usage.cacheContext.clientChanges")}</dt><dd>{sections(diagnostics.clientChanges)}</dd></div>
        <div><dt>{t("usage.cacheContext.upstreamChanges")}</dt><dd>{sections(diagnostics.upstreamChanges)}</dd></div>
      </> : null}
      {diagnostics.relayHistory.comparison !== "not_compared" ? <div>
        <dt>{t("usage.cacheContext.relayChanges")}</dt><dd>{sections(diagnostics.relayChanges)}</dd>
      </div> : null}
      <div><dt>{t("usage.cacheContext.clientHistory")}</dt><dd><History diagnostics={diagnostics.clientHistory} /></dd></div>
      <div><dt>{t("usage.cacheContext.upstreamHistory")}</dt><dd><History diagnostics={diagnostics.upstreamHistory} /></dd></div>
      <div><dt>{t("usage.cacheContext.relayHistory")}</dt><dd><History diagnostics={diagnostics.relayHistory} /></dd></div>
    </dl>
  </section>;
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
