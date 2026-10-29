import { useState } from "react";
import { Search } from "lucide-react";
import { Dialog } from "../../components/Dialog";
import { CopyButton } from "../../components/CopyButton";
import { OptionMenu } from "../../components/OptionMenu";
import { formatMoney } from "../usage/model";
import type { ComponentId } from "../usage/model";

export interface CustomerModel {
  id: string; name: string; protocols: string[]; description: string;
  rates: Record<ComponentId, bigint>; revision: string;
}

export function ModelsPage({ models, onHelp }: { models: CustomerModel[]; onHelp: () => void }) {
  const [query, setQuery] = useState("");
  const [protocol, setProtocol] = useState("all");
  const [selected, setSelected] = useState<CustomerModel | null>(null);
  const visible = models.filter(model => `${model.name} ${model.id}`.toLowerCase().includes(query.toLowerCase()) && (protocol === "all" || model.protocols.includes(protocol)));
  return <>
    <header className="page-heading"><h1>Модели</h1><span>Демонстрационный каталог · цены за 1 млн токенов</span></header>
    <div className="catalog-filters"><label className="search-field"><Search aria-hidden="true" /><input type="search" aria-label="Поиск модели" placeholder="Найти модель" value={query} onChange={event => setQuery(event.target.value)} /></label><label>Протокол модели<OptionMenu label="Протокол модели" value={protocol} onChange={setProtocol} options={[{ value: "all", label: "Все протоколы" }, ...[...new Set(models.flatMap(model => model.protocols))].map(value => ({ value, label: value }))]} /></label></div>
    <div className="table-shell"><div className="table-scroll" role="region" aria-label="Каталог моделей"><table className="catalog-table"><thead><tr><th>Модель</th><th>Протокол</th><th>Вход, USD</th><th>Чтение кэша</th><th>Выход, USD</th><th>Подробнее</th></tr></thead><tbody>{visible.map(model => <tr key={model.id}><td><strong>{model.name}</strong><small>{model.id}</small></td><td>{model.protocols.join(", ")}</td><td>{formatMoney(model.rates.input, 2)}</td><td>{formatMoney(model.rates.cacheRead, 2)}</td><td>{formatMoney(model.rates.output, 2)}</td><td><button type="button" className="text-button" aria-label={`Подробнее о ${model.name}`} onClick={() => setSelected(model)}>Открыть</button></td></tr>)}</tbody></table>{!visible.length && <p className="empty-state">Модели не найдены</p>}</div></div>
    <p className="form-note">Демонстрационный каталог с вымышленными моделями и ценами. Подключениями и доступом к моделям управляет владелец сервера.</p>
    {selected && <Dialog title="Модель и тариф" onClose={() => setSelected(null)}><div className="model-detail"><span className="status-badge">Демо-модель</span><h3>{selected.name}</h3><p>{selected.description}</p></div><dl className="detail-list"><div><dt>ID модели</dt><dd><code>{selected.id}</code></dd></div><div><dt>Протоколы</dt><dd>{selected.protocols.join(", ")}</dd></div>{([['input', 'Вход без кэша'], ['cacheRead', 'Чтение кэша'], ['cacheWrite', 'Запись кэша'], ['output', 'Выход']] as [ComponentId, string][]).map(([id, label]) => <div key={id}><dt>{label}</dt><dd>{formatMoney(selected.rates[id], 2)} USD / 1 млн</dd></div>)}<div><dt>Версия тарифа</dt><dd><code>{selected.revision}</code></dd></div></dl><CopyButton text={selected.id} label="Скопировать ID модели" /><button className="button" type="button" onClick={() => { setSelected(null); onHelp(); }}>Как подключить клиент</button><p className="form-note">Цены прошлых запросов сохранены в использовании.</p></Dialog>}
  </>;
}
