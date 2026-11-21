import { useMemo, useState } from "react";
import { OptionMenu } from "../../components/OptionMenu";
import { Pagination } from "../../components/Pagination";
import { useFormatters } from "../../i18n/format";
import { useLocale } from "../../i18n/LocaleProvider";
import { inUsagePeriod, usagePeriods } from "../usage/period";
import type { UsagePeriod } from "../usage/period";
import type { PaymentOperation } from "./BillingPage";

export function BillingHistory({ operations, asOf, onSelect }: { operations: PaymentOperation[]; asOf: string; onSelect: (operation: PaymentOperation) => void }) {
  const { t, localize } = useLocale();
  const { formatMoney, formatTime } = useFormatters();
  const [period, setPeriod] = useState<UsagePeriod>("all");
  const [kind, setKind] = useState("all"), [status, setStatus] = useState("all");
  const [page, setPage] = useState(1), [size, setSize] = useState(10);
  const kinds = useMemo(() => [...new Set(operations.map(item => item.kind))], [operations]);
  const statuses = useMemo(() => [...new Set(operations.map(item => item.status))], [operations]);
  const rows = useMemo(() => operations.filter(item => inUsagePeriod(item.at, asOf, period)
    && (kind === "all" || item.kind === kind) && (status === "all" || item.status === status))
    .sort((a, b) => Date.parse(b.at) - Date.parse(a.at)), [operations, asOf, period, kind, status]);
  const current = Math.min(page, Math.max(1, Math.ceil(rows.length / size)));
  const changed = period !== "all" || kind !== "all" || status !== "all";
  const reset = () => { setPeriod("all"); setKind("all"); setStatus("all"); setPage(1); };
  return <section className="payment-panel" aria-label={t("billing.paymentHistory")}>
    <header><h2>{t("billing.paymentHistory")}</h2>{changed && <button type="button" className="text-button" onClick={reset}>{t("billing.resetPaymentFilters")}</button>}</header>
    <div className="payment-filters">
      <OptionMenu label={t("billing.paymentPeriodUtc")} value={period} options={usagePeriods.map(item => ({ ...item, label: t(item.label), ...("shortLabel" in item ? { shortLabel: t(item.shortLabel) } : {}) }))} onChange={value => { setPeriod(value as UsagePeriod); setPage(1); }} />
      <OptionMenu label={t("billing.paymentType")} value={kind} options={[{ value: "all", label: t("billing.allOperations"), shortLabel: t("billing.operations") }, ...kinds.map(value => ({ value, label: localize(value) }))]} onChange={value => { setKind(value); setPage(1); }} />
      <OptionMenu label={t("billing.paymentStatus")} value={status} options={[{ value: "all", label: t("billing.allStatuses"), shortLabel: t("billing.statuses") }, ...statuses.map(value => ({ value, label: localize(value) }))]} onChange={value => { setStatus(value); setPage(1); }} />
    </div>
    <div className="table-scroll" role="region" aria-label={t("billing.paymentOperations")}>
      {rows.length ? <table className="payment-table"><thead><tr><th>{t("billing.dateUtc")}</th><th>{t("billing.operation")}</th><th>{t("common.status")}</th><th>{t("billing.amountUsd")}</th></tr></thead>
        <tbody>{rows.slice((current - 1) * size, current * size).map(operation => <tr key={operation.id}>
          <td>{formatTime(operation.at)}</td><td><button type="button" className="text-button" onClick={() => onSelect(operation)}>{localize(operation.kind)}</button></td>
          <td>{localize(operation.status)}</td><td>{operation.amount > 0n && "+"}{formatMoney(operation.amount, 2)}</td>
        </tr>)}</tbody></table> : <div className="payment-empty">{t(operations.length ? "billing.noPaymentsFound" : "billing.noPaymentsYet")}</div>}
    </div>
    <Pagination label={t("billing.paymentPages")} count={rows.length} page={current} size={size} sizes={[10, 25, 50]} onPage={setPage} onSize={value => { setSize(value); setPage(1); }} />
  </section>;
}
