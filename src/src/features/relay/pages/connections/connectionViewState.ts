import type { RelayMode } from "../../api/types";

export type ConnectionView = "sources" | "accounts" | "proxies" | "automations" | "remote";

export function connectionViews(mode: RelayMode, features: readonly string[]): ConnectionView[] {
  if (mode === "zenith") return ["sources"];

  const supported = new Set(features);
  const supports = (feature: string) => mode !== "remote" || supported.has(feature);
  return [
    ...(supports("accounts") ? ["accounts" as const] : []),
    ...(supports("sources") ? ["sources" as const] : []),
    ...(mode === "local" ? ["proxies" as const] : []),
    ...(supports("wake_tasks") ? ["automations" as const] : []),
    ...(mode === "remote" ? ["remote" as const] : []),
  ];
}

export function connectionInitialView(
  mode: RelayMode,
  current: ConnectionView,
  requested: string | null,
  features: readonly string[] = [],
): ConnectionView {
  if (mode === "zenith") return "sources";
  const available = connectionViews(mode, features);
  const requestedView = requested === "sources" ? "sources" : current;
  return available.includes(requestedView) ? requestedView : available[0] ?? "remote";
}

export function reconcileRemoteConnectionView(
  mode: RelayMode,
  hasRuntime: boolean,
  current: ConnectionView,
): ConnectionView {
  if (mode !== "remote" || hasRuntime) return current;
  return "remote";
}
