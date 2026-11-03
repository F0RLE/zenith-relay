import { formatSourceAmount, sourceStatsAmounts, sourceStatsStatus } from "../../../../src/src/features/relay/sourceStatsModel";
import { useLocale } from "../../i18n/LocaleProvider";
import type { OwnerSourcePresentation } from "./sourcePresentation";

export function OwnerSourceBody({ source, modelCount, valuesVisible, onModels }: {
  source?: OwnerSourcePresentation | undefined; modelCount: number; valuesVisible: boolean; onModels?: (() => void) | undefined;
}) {
  const { t, locale } = useLocale();
  const stats = source?.providerStats;
  const status = stats ? sourceStatsStatus(stats) : undefined;
  const amounts = sourceStatsAmounts(stats);
  const statusKey = { unsupported: "source.status.unsupported", unauthorized: "source.status.unauthorized", rate_limited: "source.status.rateLimited", unavailable: "source.status.unavailable", invalid_response: "source.status.invalidResponse" } as const;
  const balanceKey = { wallet: "source.balance.wallet", key_quota: "source.balance.keyQuota", subscription: "source.balance.subscription" } as const;
  const hint = stats?.stale ? t("source.dataStale") : status && status !== "available" ? t(statusKey[status]) : undefined;
  const values = (field: "balanceMicros" | "spentMicros") => amounts.filter(amount => amount[field] != null && Number.isSafeInteger(amount[field]))
    .map(amount => `${formatSourceAmount(amount[field]!, amount.currency, locale)}${amount.currency === "CREDITS" ? ` ${t("source.credits")}` : ""}`);
  const balance = status === "available" && stats?.balanceUnlimited ? [t("source.unlimited")] : values("balanceMicros");
  const spent = values("spentMicros");
  return <div className="owner-source-body">
    {valuesVisible && <dl className="owner-source-values" data-stale={stats?.stale || undefined}>
      {([[t(balanceKey[stats?.balanceKind ?? "wallet"]), balance], [t("source.spent"), spent]] as const).map(([label, lines]) => <div key={label} data-relay-tooltip={hint} tabIndex={hint ? 0 : undefined}>
        <dt>{label}</dt><dd>{lines.length ? lines.map((line, index) => <span key={index}>{line}</span>) : "—"}{hint && <span className="sr-only">{hint}</span>}</dd>
      </div>)}
    </dl>}
    <button type="button" className="owner-source-models" onClick={onModels} disabled={!onModels} aria-label={t("source.modelsCount", { count: modelCount.toLocaleString(locale) })} data-relay-tooltip={t("source.modelsAndProtocols")}>
      <span>{t("nav.models")}</span><strong>{modelCount.toLocaleString(locale)}</strong>
    </button>
  </div>;
}
