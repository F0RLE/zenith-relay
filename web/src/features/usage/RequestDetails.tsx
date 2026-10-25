import { CircleAlert, CircleCheck, Clock3, KeyRound } from "lucide-react";
import { Dialog } from "../../components/Dialog";
import { accountingLabels, executionLabels, formatDuration, formatMoney, formatSpeed, formatTime, generationSpeed } from "./model";
import type { RequestRecord } from "./model";
import { RequestTokens } from "./RequestTokens";

export function RequestDetails({ record, onClose }: { record: RequestRecord; onClose: () => void }) {
  const StatusIcon = record.execution === "completed" ? CircleCheck : record.execution === "failed" ? CircleAlert : Clock3;
  return <Dialog title="Детали запроса" onClose={onClose}>
    <section className="request-overview" aria-label="Результат запроса">
      <div className="request-identity">
        <div className="request-badges"><span className="status-badge" data-status={record.execution}><StatusIcon aria-hidden="true" />{executionLabels[record.execution]}</span><span className="status-badge" data-status={record.accounting}>Расчёт: {accountingLabels[record.accounting].toLowerCase()}</span></div>
        <h3>{record.model.name}</h3>
        <dl className="request-id"><dt>ID запроса</dt><dd><code>{record.id}</code></dd></dl>
        <div className="request-meta"><span><KeyRound aria-hidden="true" />{record.key.name} · {record.key.mask}</span><span><Clock3 aria-hidden="true" />{formatTime(record.startedAt)} UTC</span></div>
      </div>
      <dl className="request-metrics">
        <div><dt>До первого токена</dt><dd>{formatDuration(record.timing.firstTokenMs)}</dd></div>
        <div><dt>Время запроса</dt><dd>{formatDuration(record.timing.totalMs)}</dd></div>
        <div><dt>Скорость генерации</dt><dd>{formatSpeed(generationSpeed(record))}</dd></div>
        <div><dt>Итоговое списание</dt><dd data-testid="request-charge-total">{formatMoney(record.billing.postedMicroUsd)}{record.billing.postedMicroUsd !== null && <small> USD</small>}</dd></div>
      </dl>
    </section>
    {record.execution === "failed" && <section className="request-notice" data-status="failed"><h3>{record.response.result}</h3><p>{record.response.guidance}</p></section>}
    {record.execution === "running" && <section className="request-notice"><h3>Запрос выполняется</h3><p>Ожидаются итоговые токены и стоимость. Дождитесь завершения текущего запроса.</p></section>}
    {record.execution === "completed" && record.accounting === "pending" && <section className="request-notice"><h3>Ответ завершён, расчёт ожидается</h3><p>Ожидаются итоговые счётчики и сумма. Дождитесь расчёта текущего запроса.</p></section>}
    <section className="request-section"><div className="section-heading"><h3>Параметры запроса</h3></div><dl className="request-parameters">
      <div><dt>Протокол</dt><dd>{record.protocol}</dd></div><div><dt>Транспорт</dt><dd>{record.transport}</dd></div><div><dt>Скорость запроса</dt><dd>{record.tier}</dd></div><div><dt>Рассуждения</dt><dd>{record.model.reasoningEffort}</dd></div>
    </dl></section>
    <RequestTokens record={record} />
  </Dialog>;
}
