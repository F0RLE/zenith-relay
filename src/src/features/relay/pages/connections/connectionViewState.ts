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
): ConnectionView {
  if (mode === "zenith") return "sources";
  return requested === "sources" || current === "sources" ? "sources" : "accounts";
}

export function reconcileRemoteConnectionView(
  mode: RelayMode,
  hasRuntime: boolean,
  current: ConnectionView,
): ConnectionView {
  if (mode !== "remote" || hasRuntime) return current;
  return "remote";
}
