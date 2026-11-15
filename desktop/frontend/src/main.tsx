import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app/App";
import { RelayErrorBoundary } from "./app/RelayErrorBoundary";
import { initI18n } from "./i18n";
import { getSystemLocale, recordPerformance } from "./platform/desktop";
import { relayCommands } from "./features/relay/api/commands";
import { redactFeedbackText } from "./features/relay/state/feedback";
import { consumeInterruptedAccountImportConfirmation } from "./features/relay/state/relayPreferences";
import { setStartupPhase } from "./platform/startup";

// Renderer failures used to disappear with the WebView.  Keep a small,
// redacted breadcrumb in Relay's native diagnostics directory so a startup or
// import failure can be investigated after the window is gone.
let lastReportedRendererError = "";
let lastReportedRendererErrorAt = 0;
function reportRendererError(source: string, errorValue: unknown, stack?: string) {
  const message = redactFeedbackText(
    errorValue instanceof Error ? errorValue.message : String(errorValue),
  );
  if (!message) return;
  const now = Date.now();
  const fingerprint = `${source}:${message}`;
  if (fingerprint === lastReportedRendererError && now - lastReportedRendererErrorAt < 2_000) return;
  lastReportedRendererError = fingerprint;
  lastReportedRendererErrorAt = now;
  void relayCommands.recordFrontendDiagnostic({
    source,
    message,
    ...(stack ? { stack: redactFeedbackText(stack) } : {}),
    fatal: source === "window-error" || source === "bootstrap",
  }).catch(() => undefined);
}

window.addEventListener("error", (event) => {
  reportRendererError("window-error", event.error ?? event.message, event.error?.stack);
});
window.addEventListener("unhandledrejection", (event) => {
  const rejectionReason = event.reason;
  reportRendererError("unhandled-rejection", rejectionReason, rejectionReason instanceof Error ? rejectionReason.stack : undefined);
});

function reportInterruptedAccountImportConfirmation() {
  if (!consumeInterruptedAccountImportConfirmation()) return;
  void relayCommands.recordFrontendDiagnostic({
    source: "account-import",
    operation: "confirm_local_account_import",
    code: "confirmation_interrupted",
    message: "account import confirmation did not return before the previous renderer stopped",
  }).catch(() => undefined);
}

void bootstrap().catch((error: unknown) => {
  reportRendererError("bootstrap", error, error instanceof Error ? error.stack : undefined);
  setStartupPhase("error");
});

async function bootstrap() {
  reportInterruptedAccountImportConfirmation();
  const systemLocale = getSystemLocale().catch(() => null);
  await initI18n(navigator.language);
  void systemLocale.then((locale) => {
    if (locale) void initI18n(locale);
  });
  performance.mark("zenith:i18n-ready");
  performance.measure("zenith:i18n", "zenith:html-start", "zenith:i18n-ready");
  setStartupPhase("runtime");

  ReactDOM.createRoot(document.getElementById("root")!).render(
    <React.StrictMode>
      <RelayErrorBoundary>
        <App />
      </RelayErrorBoundary>
    </React.StrictMode>,
  );
  performance.mark("zenith:react-rendered");
  requestAnimationFrame(() => requestAnimationFrame(() => {
    performance.mark("zenith:first-frame");
    const measure = performance.measure("zenith:first-frame", "zenith:html-start", "zenith:first-frame");
    void recordPerformance("first_frame", measure.duration, "startup");
  }));
}
