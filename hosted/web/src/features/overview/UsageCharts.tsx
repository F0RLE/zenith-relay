import { Activity, CreditCard, Database } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import { formatMoney } from "../usage/model";
import { localInterval } from "../usage/ReportTools";
import { Chart } from "./OverviewChart";
import "./overview-charts.css";

type Count = number | bigint;
type Metrics = { requests: Count; samples: Count; partial: Count; unsettled: Count; tokens: bigint | null; spend: bigint };
type Bucket = Metrics & { from: number; to: number };

export function UsageCharts({ data, intervalLink, chargeLabel }: { data: { totals: Metrics; buckets: Bucket[] };
  intervalLink?: (bucket: number) => string; chargeLabel?: string }) {
  const { t, locale } = useLocale();
  const money = (n: bigint) => formatMoney(n, n % 10000n === 0n ? 2 : 6, locale) + " USD";
  const count = (n: Count | null) => n === null ? "—" : n.toLocaleString(locale);
  const totals = data.totals;
  const charges = chargeLabel ?? t("report.postedCharges");
  const buckets = data.buckets.map((b, bucket) => ({ ...b, startMs: b.from, interval: localInterval(b.from, b.to, locale, true),
    href: b.requests ? intervalLink?.(bucket) : undefined }));
  return <>
    <div className="overview-chart-stack report-chart-stack">
      <Chart buckets={buckets} icon={Database} label={t("report.recordedTokens")} total={count(totals.tokens)}
        note={totals.samples < totals.requests || totals.partial > 0
          ? t("report.tokenCoverage", { known: count(totals.samples), count: count(totals.requests), partial: count(totals.partial) }) : undefined}
        unit="tokens" tone="tokens" measure={b => b.tokens === null ? null : Number(b.tokens)} exact={b => count(b.tokens)} />
      <Chart buckets={buckets} icon={CreditCard} label={charges} total={money(totals.spend)}
        note={totals.unsettled > 0 ? t("usage.unsettledCount", { count: count(totals.unsettled) }) : undefined}
        unit="USD" tone="cost" measure={b => Number(b.spend) / 1000000} exact={b => money(b.spend)} />
      <Chart buckets={buckets} icon={Activity} label={t("common.requests")} total={count(totals.requests)}
        unit="requests" tone="requests" measure={b => Number(b.requests)} exact={b => count(b.requests)} />
    </div>
    <div className="sr-only"><table><caption>{t("overview.chartData")}</caption>
      <thead><tr>{[t("overview.intervalUtc"), t("common.requests"), t("report.recordedTokens"), charges]
        .map((label, index) => <th scope="col" key={index}>{label}</th>)}</tr></thead>
      <tbody>{buckets.map(b => <tr key={b.from}><th scope="row">{b.interval}</th><td>{count(b.requests)}</td>
        <td>{count(b.tokens)}</td><td>{money(b.spend)}</td></tr>)}</tbody>
    </table></div>
  </>;
}
