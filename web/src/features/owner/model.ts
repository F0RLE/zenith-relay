import type { OwnerAccountPresentation } from "./accountPresentation";
import type { OwnerProxy } from "./proxyPresentation";
import { sourceEndpoint, validSourceConfiguration } from "./sourcePresentation";
import type { OwnerSourceConfiguration, OwnerSourcePresentation } from "./sourcePresentation";
import type { OwnerMemberSettings } from "./poolMemberSettings";
import { validMemberSettings } from "./poolMemberSettings";
import type { OwnerPoolActivity } from "./poolActivity";
import type { OwnerModelRules } from "./modelRules";

export interface OwnerConnection {
  id: string; name: string; family: string; kind: "account" | "api";
  models: string[]; inPool: boolean; paused: boolean; trashed: boolean;
  account?: OwnerAccountPresentation;
  source?: OwnerSourcePresentation;
  modelPolicy?: { allowedModels: string[]; excludedModels: string[] };
}
export interface OwnerCustomer { id: string; name: string; status: string; keys: number }
export interface OwnerEvent { id: string; at: string; action: string; target: string }
export interface OwnerSnapshot { connections: OwnerConnection[]; customers: OwnerCustomer[]; events: OwnerEvent[]; proxies?: OwnerProxy[]; poolActivity?: OwnerPoolActivity; modelRules?: OwnerModelRules; modelDisplayOrder?: string[] }

export type ConnectionAction =
  | { type: "add"; connection: OwnerConnection }
  | { type: "addMembers"; ids: string[] }
  | { type: "saveSource"; id: string; configuration: OwnerSourceConfiguration }
  | { type: "saveMember"; id: string; settings: OwnerMemberSettings }
  | { type: "pool" | "pause" | "trash" | "restore"; id: string }
  | { type: "move"; id: string; direction: -1 | 1 };

// Only the explicitly injected demo store calls this reducer. No admission,
// authentication or service-state claim is made without a future server adapter.
export function updateConnections(current: OwnerConnection[], action: ConnectionAction): OwnerConnection[] {
  if (action.type === "saveMember") {
    if (!validMemberSettings(action.settings)) return current;
    const member = current.find(item => item.id === action.id);
    if (!member || member.kind !== "account" || !member.inPool || member.trashed) return current;
    const { allowedModels, excludedModels, paused, purchaseCostMicroUsd } = action.settings;
    return current.map(item => item !== member ? item : {
      ...item, paused,
      modelPolicy: { allowedModels: [...allowedModels], excludedModels: [...excludedModels] },
      account: {
        plan: null, route: null, quota: { primary: null, secondary: null }, quotaStatus: "pending",
        subscriptionUntilMs: null, apiEquivalent: null, quotaWindowUsage: null,
        ...item.account, purchaseCostMicroUsd,
      },
    });
  }
  if (action.type === "addMembers") {
    const ids = new Set(action.ids);
    let changed = false;
    const next = current.map(item => {
      if (!ids.has(item.id) || item.inPool || item.trashed) return item;
      changed = true;
      return { ...item, inPool: true };
    });
    return changed ? next : current;
  }
  if (action.type === "saveSource") {
    if (!validSourceConfiguration(action.configuration)) return current;
    const previous = current.find(item => item.id === action.id);
    if (previous && (previous.kind !== "api" || previous.trashed)) return current;
    const { name, baseUrl, wireApi, models } = action.configuration;
    const targetChanged = sourceEndpoint(previous?.source?.baseUrl) !== sourceEndpoint(baseUrl) || previous?.source?.wireApi !== wireApi;
    const modelsChanged = previous?.models.join("\n") !== models.join("\n");
    const connection: OwnerConnection = {
      ...(previous ?? { id: action.id, kind: "api", family: "connections.apiProvider", inPool: false, paused: false, trashed: false }),
      name: name.trim(), models: [...models],
      source: targetChanged
        ? { baseUrl, wireApi, resolvedProtocolBindings: [], providerStats: null }
        : { ...previous!.source!, ...(modelsChanged ? { resolvedProtocolBindings: (previous!.source!.resolvedProtocolBindings ?? []).map(binding => ({ ...binding, modelIds: binding.modelIds.filter(id => models.some(model => model.toLowerCase() === id.toLowerCase())) })) } : {}) },
    };
    return previous ? current.map(item => item.id === action.id ? connection : item) : [...current, connection];
  }
  if (action.type === "add") return [...current, { ...action.connection, inPool: false, paused: false, trashed: false }];
  if (action.type === "move") {
    const positions = current.flatMap((item, index) => item.inPool && !item.trashed ? [index] : []);
    const index = current.findIndex(item => item.id === action.id);
    const poolIndex = positions.indexOf(index);
    if (poolIndex < 0) return current;
    const target = positions[poolIndex + action.direction];
    if (target === undefined) return current;
    const copy = [...current];
    [copy[index], copy[target]] = [copy[target]!, copy[index]!];
    return copy;
  }
  return current.map(item => {
    if (item.id !== action.id) return item;
    if (action.type === "trash") return { ...item, trashed: true, inPool: false };
    if (action.type === "restore") return { ...item, trashed: false, inPool: false };
    if (item.trashed) return item;
    if (action.type === "pool") return { ...item, inPool: !item.inPool };
    return { ...item, paused: !item.paused };
  });
}
