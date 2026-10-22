export type ConnectionView = "sources" | "accounts" | "proxies" | "automations" | "trash";

export function connectionViews(): ConnectionView[] {
  return ["accounts", "sources", "proxies", "automations", "trash"];
}

export function connectionInitialView(
  currentView: ConnectionView,
  requested: string | null,
): ConnectionView {
  const available = connectionViews();
  const requestedView = requested === "sources" ? "sources" : currentView;
  return available.includes(requestedView) ? requestedView : "accounts";
}
