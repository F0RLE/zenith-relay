import { useTranslation } from "react-i18next";
import type { ApiEquivalentSummary } from "../../api/types";
import { formatFullNumber } from "../../usageTotals";
import { usageBreakdown } from "./usageBreakdown";
import type { UsageRow } from "./usageData";
import { formatUsageApiEquivalent } from "./usageFormatting";

export function RequestTokenLedger({ row }: { row: UsageRow }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const usage = usageBreakdown({ ...row, totalTokens: row.tokens });
  const { apiEquivalent: price, apiEquivalentBreakdown: parts, imageUsage: image } = row;
  const hasPrice = price && (price.microUsd > 0 || price.pricedTokens > 0);
  if (!hasPrice && !image && Object.values(usage).every((v) => v == null)) return null;

  function line(label: string, count: number | null | undefined, cost?: ApiEquivalentSummary | null, kind: "group" | "included" | "part" = "part") {
    // A per-image fee can be known even when the token count is absent.
    if (count == null && !(cost && cost.microUsd > 0)) return null;
    const included = kind === "included";
    const hint = included ? t(label === "imageCachedInput" ? "usage.imageCacheIncluded" : "usage.outputPriceIncludedHint")
      : cost ? t("usage.requestApiEquivalentHint", { count: cost.unpricedTokens }) : t("usage.tokenPriceUnavailable");
    return <tr className={kind === "group" ? "request-details-token-group-heading" : "request-details-token-child"}>
      <th scope="row">{t(`usage.${label}`)}</th>
      <td className="request-token-count">{count == null ? "—" : formatFullNumber(count, locale)}</td>
      <td className={`request-token-cost${included ? " request-token-included" : ""}`} data-relay-tooltip={hint}>
        {included ? t("usage.outputPriceIncluded") : cost ? formatUsageApiEquivalent(cost, locale, 6) : "—"}
      </td>
    </tr>;
  }
  return <div className="request-token-details">
    <table className="request-ledger">
      <caption>{t("usage.requestSections.ledger")}</caption>
      <thead><tr><th scope="col">{t("usage.requestSections.category")}</th><th scope="col">{t("usage.tokens")}</th>
        <th scope="col">{t("usage.apiEquivalent")}</th></tr></thead>
      <tbody>
        {line("inputTokens", usage.inputTotal, parts?.input, "group")}
        {image ? <>
          {line("imageTextInput", image.inputTextTokens, parts?.image?.inputText)}
          {line("imageImageInput", image.inputImageTokens, parts?.image?.inputImage)}
          {line("imageSvgInput", image.svgUsage?.inputTokens, parts?.image?.svgInput)}
          {line("imageCachedInput", image.cachedInputTokens, null, "included")}
        </> : <>
          {usage.uncachedInput != null && usage.uncachedInput > 0 ? line("uncachedInputTokens", usage.uncachedInput, parts?.uncachedInput) : null}
          {line("cachedInputTokens", usage.cacheRead, parts?.cacheRead)}
          {line("cacheWriteInputTokens", usage.cacheWrite, parts?.cacheWrite)}
        </>}
      </tbody>
      <tbody>
        {line("outputTokens", usage.outputTotal, parts?.output, "group")}
        {image ? <>
          {line("imageImageOutput", image.outputImageTokens, parts?.image?.outputImage)}
          {image.outputTextTokens ? line("imageTextOutput", image.outputTextTokens, parts?.image?.outputText) : null}
          {line("imageSvgOutput", image.svgUsage?.outputTokens, parts?.image?.svgOutput)}
        </> : null}
        {usage.reasoning != null && usage.reasoning > 0 ? <>
          {line("visibleOutputTokens", usage.visibleOutput, null, "included")}
          {line("reasoningTokens", usage.reasoning, null, "included")}
        </> : null}
      </tbody>
      {usage.total != null || hasPrice ? <tfoot><tr>
        <th scope="row">{t("usage.requestSections.total")}</th>
        <td className="request-token-total">{usage.total == null ? "—" : formatFullNumber(usage.total, locale)}</td>
        <td>{hasPrice ? <span className="request-details-cost-value" data-relay-tooltip={t("usage.requestApiEquivalentHint", { count: price.unpricedTokens })}>
          {formatUsageApiEquivalent(price, locale, 6)}</span> : "—"}</td>
      </tr></tfoot> : null}
    </table>
  </div>;
}
