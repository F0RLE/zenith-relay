import { CopyButton } from "../../components/CopyButton";

const example = '{\n  "baseURL": "https://relay.example.test/v1",\n  "apiKey": "<API_KEY>",\n  "model": "<MODEL_ID>"\n}';
export function HelpPanel() {
  return <>
    <section className="help-section"><h3>Подключение клиента</h3><ol><li>Получите адрес Hosted API у владельца сервера.</li><li>Создайте личный API-ключ в профиле.</li><li>Выберите модель и поддерживаемый клиентом протокол.</li><li>Укажите адрес, ключ и точный ID модели в своём клиенте.</li></ol></section>
    <section className="help-section"><h3>Пример конфигурации</h3><pre className="code-example">{example}</pre><CopyButton text={example} label="Скопировать пример" /><p className="form-note">Адрес example.test, модель и ключ — заглушки. Это не рабочая конфигурация; демо ничего не отправляет.</p></section>
    <section className="help-section"><h3>Где искать информацию</h3><dl className="detail-list"><div><dt>Использование</dt><dd>Запросы, токены, стоимость, скорость и ошибки по ключам.</dd></div><div><dt>Баланс</dt><dd>Пополнения, возвраты и корректировки — без построчных списаний за API.</dd></div><div><dt>Ошибка запроса</dt><dd>Откройте детали и скопируйте безопасную сводку. Не отправляйте ключ или текст запроса.</dd></div></dl></section>
  </>;
}
