import type { RuntimeSnapshot } from "../api/types";
import { createDisplaySnapshotCache } from "./displaySnapshotCache";

function validRuntime(value: unknown): value is RuntimeSnapshot {
  if (!value || typeof value !== "object") return false;
  const snapshot = value as RuntimeSnapshot;
  const lists = [snapshot.sources, snapshot.accounts, snapshot.automations, snapshot.wakeHistory, snapshot.warnings];
  return typeof snapshot.schemaVersion === "number" && typeof snapshot.platform === "string"
    && lists.every(Array.isArray) && Array.isArray(snapshot.capabilities?.features)
    && typeof snapshot.gateway?.running === "boolean" && Array.isArray(snapshot.gateway.visibleModelIds)
    && snapshot.sources.every((source) => source && typeof source.id === "string"
      && Array.isArray(source.models) && Array.isArray(source.allowedModels) && Array.isArray(source.excludedModels)
      && typeof source.baseUrl === "string" && source.apiEquivalent != null)
    && snapshot.accounts.every((account) => account && typeof account.id === "string"
      && Array.isArray(account.models) && Array.isArray(account.allowedModels) && Array.isArray(account.excludedModels)
      && account.quota != null && account.subscription != null && account.authState != null && account.apiEquivalent != null);
}

export const runtimePreview = createDisplaySnapshotCache("runtime", 1, validRuntime);

export function rememberRuntimePreview(snapshot: RuntimeSnapshot) {
  // Persist the original redacted DTO, before identity reveal and event overlays.
  // A previous process's activity/leases are never restored as live requests.
  runtimePreview.remember("local", {
    ...snapshot,
    gateway: { ...snapshot.gateway, routingOrder: [] },
  });
}
