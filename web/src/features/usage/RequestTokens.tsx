import { componentTokens, componentTotal, tokenTotal } from "./model";
import type { ComponentId, RequestRecord } from "./model";
import { useLocale } from "../../i18n/LocaleProvider";
import { useFormatters } from "../../i18n/format";

export function RequestTokens({ record }: { record: RequestRecord }) {
  const { t } = useLocale();
  const { formatInteger, formatMoney } = useFormatters();
  const { tokens, billing } = record;
  const quantities = componentTokens(tokens);
  const total = tokenTotal(tokens);
  const sum = componentTotal(billing.components);
  const mismatch = sum !== null && billing.postedMicroUsd !== null && sum !== billing.postedMicroUsd;
  const chargeLine = (id: ComponentId, label: string) => <div className="token-row" key={id}>
    <dt>{label}</dt><dd>{formatInteger(quantities[id])}</dd>
    <dd className="token-charge" aria-label={t("request.componentCost", { label })}>{formatMoney(billing.components[id])}{billing.components[id] !== null && " $"}</dd>
  </div>;

  const inputCost = componentTotal({ ...billing.components, output: 0n });
  return <section className="request-token-details" aria-label={t("common.tokens")}>
    <div className="request-token-header">
      <dl className="request-token-total"><div><dt>{t("usage.totalTokens")}</dt><dd>{formatInteger(total)}</dd></div></dl>
      <span aria-hidden="true">{t("request.quantity")}</span><span aria-hidden="true">{t("request.cost")}</span>
    </div>
    <div className="token-grid">
      <dl className="token-group">
        <div className="token-heading"><dt>{t("usage.inputTokens")}</dt><dd>{formatInteger(tokens.input)}</dd><dd className="token-charge">{formatMoney(inputCost)}{inputCost !== null && " $"}</dd></div>
        {chargeLine("input", t("request.standardInput"))}{chargeLine("cacheRead", t("request.readFromCache"))}{chargeLine("cacheWrite", t("request.writtenToCache"))}
      </dl>
      <dl className="token-group">
        <div className="token-heading"><dt>{t("usage.outputTokens")}</dt><dd>{formatInteger(tokens.output)}</dd><dd className="token-charge">{formatMoney(billing.components.output)}{billing.components.output !== null && " $"}</dd></div>
        <div className="token-row"><dt>{t("request.visibleOutput")}</dt><dd>{formatInteger(tokens.output === null || tokens.reasoning === null ? null : tokens.output - tokens.reasoning)}</dd><dd className="token-included">{t("request.included")}</dd></div>
        <div className="token-row"><dt>{t("request.reasoning")}</dt><dd>{formatInteger(tokens.reasoning)}</dd><dd className="token-included">{t("request.included")}</dd></div>
      </dl>
    </div>
    {mismatch && <p role="alert" className="error-note">{t("request.costMismatch")}</p>}
  </section>;
}
