import { componentTokens, componentTotal, formatInteger, formatMoney, tokenTotal } from "./model";
import type { ComponentId, RequestRecord } from "./model";

export function RequestTokens({ record }: { record: RequestRecord }) {
  const { tokens, billing } = record;
  const quantities = componentTokens(tokens);
  const total = tokenTotal(tokens);
  const sum = componentTotal(billing.components);
  const mismatch = sum !== null && billing.postedMicroUsd !== null && sum !== billing.postedMicroUsd;
  const chargeLine = (id: ComponentId, label: string) => <div className="token-row" key={id}>
    <dt>{label}</dt><dd>{formatInteger(quantities[id])}</dd>
    <dd className="token-charge" aria-label={`${label}: стоимость`}>{formatMoney(billing.components[id])}<small> USD</small></dd>
    <span className="token-rate">{formatMoney(billing.rates[id], 2)} USD / 1 млн токенов</span>
  </div>;

  return <section className="request-section" aria-labelledby="request-tokens-title">
    <div className="section-heading"><h3 id="request-tokens-title">Токены</h3><span>{total === null ? "Итоговые данные ожидаются" : <>Всего <strong>{formatInteger(total)}</strong></>}</span></div>
    <div className="token-grid">
      <dl className="token-group">
        <div className="token-heading"><dt>Вход</dt><dd>{formatInteger(tokens.input)}</dd><dd className="token-charge">{formatMoney(componentTotal({ ...billing.components, output: 0n }))}<small> USD</small></dd></div>
        {chargeLine("input", "Без кэша")}{chargeLine("cacheRead", "Чтение кэша")}{chargeLine("cacheWrite", "Запись кэша")}
      </dl>
      <dl className="token-group">
        <div className="token-heading"><dt>Выход</dt><dd>{formatInteger(tokens.output)}</dd><dd className="token-charge">{formatMoney(billing.components.output)}<small> USD</small></dd></div>
        <div className="token-row"><dt>Ответ модели</dt><dd>{formatInteger(tokens.output === null || tokens.reasoning === null ? null : tokens.output - tokens.reasoning)}</dd><dd className="token-included">Входит в выход</dd></div>
        <div className="token-row"><dt>Рассуждения</dt><dd>{formatInteger(tokens.reasoning)}</dd><dd className="token-included">Входит в выход</dd></div>
        <div className="token-output-rate">{formatMoney(billing.rates.output, 2)} USD / 1 млн выходных токенов</div>
      </dl>
    </div>
    {mismatch && <p role="alert" className="error-note">Компоненты стоимости не совпадают с проведённым списанием. Итог не пересчитан: сохраните ID запроса для поддержки.</p>}
  </section>;
}
