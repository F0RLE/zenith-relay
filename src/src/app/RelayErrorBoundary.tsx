import { Component, type ErrorInfo, type ReactNode } from "react";
import { relayCommands } from "../features/relay/api/commands";
import { redactFeedbackText } from "../features/relay/state/feedback";

type Props = { children: ReactNode };
type State = { hasError: boolean };

/** Keeps one malformed snapshot or renderer component from blanking the whole app. */
export class RelayErrorBoundary extends Component<Props, State> {
  public override state: State = { hasError: false };

  public static getDerivedStateFromError(): State {
    return { hasError: true };
  }

  public override componentDidCatch(error: unknown, info: ErrorInfo) {
    const message = redactFeedbackText(error instanceof Error ? error.message : String(error));
    const stack = error instanceof Error && error.stack ? redactFeedbackText(error.stack) : undefined;
    const componentStack = redactFeedbackText(info.componentStack || "");
    void relayCommands.recordFrontendDiagnostic({
      source: "react-error-boundary",
      operation: "render",
      code: "render_failed",
      message: message || "renderer failed",
      ...(stack || componentStack ? { stack: [stack, componentStack].filter(Boolean).join("\n") } : {}),
      fatal: true,
    }).catch(() => undefined);
  }

  public override render() {
    if (!this.state.hasError) return this.props.children;
    const russian = document.documentElement.lang.startsWith("ru");
    return <section className="relay-render-error" role="alert">
      <h1>{russian ? "Relay не смог отобразить этот экран" : "Relay could not render this screen"}</h1>
      <p>{russian ? "Ошибка записана в папку диагностики. Перезагрузите приложение и повторите действие." : "The error was saved in diagnostics. Reload the app and try the action again."}</p>
      <button type="button" onClick={() => window.location.reload()}>{russian ? "Перезагрузить" : "Reload"}</button>
    </section>;
  }
}
