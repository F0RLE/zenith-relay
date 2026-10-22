import { createRoot } from "react-dom/client";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <main className="integration-unavailable">
    <h1>Relay Hosted</h1>
    <p>Подключение кабинета к Hosted API ещё не реализовано.</p>
    <p>Вход, баланс и использование недоступны. Запросы на сервер не отправляются.</p>
    {import.meta.env.DEV && <a href="/preview.html">Открыть локальный UI preview с вымышленными данными</a>}
  </main>,
);
