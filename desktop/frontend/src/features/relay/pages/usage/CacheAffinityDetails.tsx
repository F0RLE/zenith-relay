import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { CacheAffinityDiagnostics, CacheAffinityMember } from "../../api/types";

export function CacheAffinityDetails({ diagnostics }: { diagnostics: CacheAffinityDiagnostics | null | undefined }) {
  const { t, i18n } = useTranslation();
  if (!diagnostics) return null;
  const number = (value: number) => value.toLocaleString(i18n.language, { maximumFractionDigits: 2 });
  const quota = (value: number | null) => value == null ? "—" : `${number(value / 100)}%`;
  const load = (member: CacheAffinityMember) => `${number(member.inFlight)}/${number(member.effectiveCapacity)}`;
  const credits = (member: CacheAffinityMember) => member.providerCreditsUnlimited ? "∞"
    : member.providerCreditsMicroUnits == null ? "—" : number(member.providerCreditsMicroUnits / 1_000_000);
  const field = (key: string, value: ReactNode) => <div className="request-cache-affinity" key={key}><dt>{t(key)}</dt><dd>{value}</dd></div>;
  const owner = diagnostics.preferred;
  return <>
    {field("usage.cacheAffinity.decision", <>
      {t(`usage.cacheAffinity.reasons.${diagnostics.reason}`)}
      {diagnostics.ownerBlockReason ? <> · {t(`usage.cacheAffinity.blocks.${diagnostics.ownerBlockReason}`)}</> : null}
      {diagnostics.selection === "warm_alternative" ? <> · {t("usage.cacheAffinity.warmAlternative")}</> : null}
    </>)}
    {owner && diagnostics.selection !== "preferred" ? <>
      {field("usage.cacheAffinity.quota", `${quota(owner.quotaRemainingBasisPoints)} → ${quota(diagnostics.selected.quotaRemainingBasisPoints)}`)}
      {field("usage.cacheAffinity.load", `${load(owner)} → ${load(diagnostics.selected)}`)}
    </> : null}
    {diagnostics.reason === "quota_headroom" ? field("usage.cacheAffinity.quotaThreshold", t("usage.cacheAffinity.quotaComparison", {
      best: quota(diagnostics.bestQuotaRemainingBasisPoints), margin: number(diagnostics.quotaSwitchMarginBasisPoints / 100),
    })) : null}
    {diagnostics.reason === "provider_credits" && owner ? field("usage.cacheAffinity.credits", t("usage.cacheAffinity.creditComparison", {
      owner: credits(owner), selected: credits(diagnostics.selected), margin: number(diagnostics.providerCreditSwitchMarginMicroUnits / 1_000_000),
    })) : null}
    {diagnostics.reason === "parallel_load" ? field("usage.cacheAffinity.loadThreshold", number(diagnostics.loadAllowanceRequests)) : null}
  </>;
}
