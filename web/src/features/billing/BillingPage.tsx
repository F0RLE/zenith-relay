import { useState } from "react";
import { Plus } from "lucide-react";
import { Dialog } from "../../components/Dialog";
import { formatMoney, formatTime } from "../usage/model";

export interface PaymentOperation { id: string; at: string; kind: string; amount: bigint; status: string }
export function BillingPage({ balance, reserved, operations, onTopUp }: { balance: bigint; reserved: bigint; operations: PaymentOperation[]; onTopUp: (microUsd: bigint) => void }) {
  const [dialog, setDialog] = useState<"amount" | "confirm" | PaymentOperation | null>(null);
  const [amount, setAmount] = useState("25");
  return <>
    <header className="page-heading"><h1>Баланс</h1><span>Только платёжные операции</span></header>
    <section className="balance-panel" aria-label="Демо-баланс"><div><div><p>Доступный баланс</p><strong>{formatMoney(balance, 2)} USD</strong></div><button type="button" className="button primary-button" onClick={() => { setAmount("25"); setDialog("amount"); }}><Plus aria-hidden="true" />Пополнить</button></div><p>Зарезервировано под запросы: {formatMoney(reserved, 2)} USD</p></section>
    <section className="payment-panel"><header><h2>Операции</h2><span>Пополнения, возвраты и корректировки</span></header><div className="table-scroll" role="region" aria-label="Платёжные операции"><table className="payment-table"><thead><tr><th>Дата, UTC</th><th>Операция</th><th>Статус</th><th>Сумма, USD</th></tr></thead><tbody>{operations.map(operation => <tr key={operation.id}><td>{formatTime(operation.at)}</td><td><button type="button" className="text-button" onClick={() => setDialog(operation)}>{operation.kind}</button></td><td>{operation.status}</td><td>{operation.amount > 0n && "+"}{formatMoney(operation.amount, 2)}</td></tr>)}</tbody></table></div></section>
    <p className="form-note">Списания по запросам и API-ключам — во вкладке «Использование».</p>
    {dialog && <Dialog title={typeof dialog === "object" ? "Платёжная операция" : "Демо-пополнение"} onClose={() => setDialog(null)}>
      {typeof dialog === "object" ? <><dl className="detail-list"><div><dt>ID операции</dt><dd><code>{dialog.id}</code></dd></div><div><dt>Тип</dt><dd>{dialog.kind}</dd></div><div><dt>Сумма</dt><dd>{formatMoney(dialog.amount, 2)} USD</dd></div><div><dt>Статус</dt><dd>{dialog.status}</dd></div></dl><p className="form-note">Демонстрационная операция.</p></> : dialog === "amount" ? <form className="demo-form" onSubmit={event => { event.preventDefault(); if (Number.isSafeInteger(Number(amount)) && Number(amount) >= 1 && Number(amount) <= 1000) { setAmount(String(Number(amount))); setDialog("confirm"); } }}><label>Сумма, USD<input type="number" min="1" max="1000" step="1" value={amount} required onChange={event => setAmount(event.target.value)} /></label><p className="form-note">Демо: целая сумма от 1 до 1000 USD.</p><button type="submit" className="button primary-button">Посмотреть подтверждение</button></form> : <><dl className="detail-list"><div><dt>Демо-пополнение</dt><dd>{formatMoney(BigInt(amount) * 1000000n, 2)} USD</dd></div></dl><p>Демо-пополнение сохранится до перезагрузки страницы.</p><button type="button" className="button primary-button" onClick={() => { onTopUp(BigInt(amount) * 1000000n); setDialog(null); }}>Добавить демо-пополнение</button></>}
    </Dialog>}
  </>;
}
