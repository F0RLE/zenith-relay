import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { initI18n } from "./i18n";
import { getSystemLocale, recordPerformance, revealWindowAfterBackgroundColor } from "./platform/desktop";

const STARTUP_REVEAL_FALLBACK_MS = 10_000;
let startupFallbackTimer: number | undefined;

function revealStartupShell() {
  if (document.documentElement.dataset.startupReady === "true") return;
  if (startupFallbackTimer !== undefined) window.clearTimeout(startupFallbackTimer);
  const splash = document.getElementById("splash-screen");
  if (splash && window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    splash.remove();
  } else if (splash) {
    const removeAfterFade = (event: TransitionEvent) => {
      if (event.target !== splash || event.propertyName !== "opacity") return;
      splash.removeEventListener("transitionend", removeAfterFade);
      splash.remove();
    };
    splash.addEventListener("transitionend", removeAfterFade);
  }
  document.documentElement.dataset.startupReady = "true";
}

window.addEventListener("zenith-startup-ready", revealStartupShell, { once: true });
const initialTheme = document.documentElement.dataset.theme;
const initialThemeIsDark = initialTheme === "dark" || (
  initialTheme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches
);
void revealWindowAfterBackgroundColor(initialThemeIsDark ? "#121719" : "#f2f5f6");
void bootstrap();

async function bootstrap() {
  startupFallbackTimer = window.setTimeout(revealStartupShell, STARTUP_REVEAL_FALLBACK_MS);
  const systemLocale = getSystemLocale().catch(() => null);
  await initI18n(navigator.language);
  void systemLocale.then((locale) => {
    if (locale) void initI18n(locale);
  });
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
