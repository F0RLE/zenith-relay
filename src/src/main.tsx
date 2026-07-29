import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { initI18n } from "./i18n";
import { getSystemLocale, recordPerformance } from "./platform/desktop";

void bootstrap();

async function bootstrap() {
  const systemLocale = await getSystemLocale().catch(() => navigator.language);
  await initI18n(systemLocale);
  performance.mark("zenith:i18n-ready");
  performance.measure("zenith:i18n", "zenith:html-start", "zenith:i18n-ready");

  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
  performance.mark("zenith:react-rendered");
  requestAnimationFrame(() => requestAnimationFrame(() => {
    performance.mark("zenith:first-frame");
    const measure = performance.measure("zenith:first-frame", "zenith:html-start", "zenith:first-frame");
    void recordPerformance("first_frame", measure.duration, "startup");
  }));
}
