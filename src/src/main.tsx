import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { initI18n } from "./i18n";
import { getSystemLocale } from "./tauri";

void bootstrap();

async function bootstrap() {
  const systemLocale = await getSystemLocale().catch(() => navigator.language);
  await initI18n(systemLocale);

  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
