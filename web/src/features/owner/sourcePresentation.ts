import type { SourceSummary, SourceWireApi } from "../../../../src/src/features/relay/api/types/source";
import { effectiveSourceProtocolBindings } from "../../../../src/src/features/relay/sourceProtocolBindings";

/** Safe host projection; never include a provider key or native credentials. */
export type OwnerSourcePresentation = Pick<SourceSummary,
  "baseUrl" | "wireApi" | "protocolBindings" | "resolvedProtocolBindings" | "providerStats" | "refreshState"
>;

export const sourceProtocolLabels = {
  responses: "Responses", chat_completions: "Chat Completions", messages: "Messages",
  gemini: "Gemini", gemini_interactions: "Gemini Interactions",
} as const satisfies Record<SourceWireApi, string>;

/** Display only an endpoint, never credentials, query tokens or fragments. */
export function sourceEndpoint(value: string | undefined) {
  try {
    const url = new URL(value ?? "");
    return ["https:", "http:"].includes(url.protocol) ? `${url.origin}${url.pathname.replace(/\/$/, "")}` : null;
  } catch { return null; }
}

export function validSourceEndpoint(value: string) {
  try {
    const url = new URL(value.trim());
    return ["https:", "http:"].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash;
  } catch { return false; }
}

export interface OwnerSourceConfiguration { name: string; baseUrl: string; wireApi: SourceWireApi; models: string[] }

export function validSourceModelId(value: string) {
  return value.length > 0 && value.length <= 200 && !/\s/.test(value);
}

export function validSourceConfiguration(value: OwnerSourceConfiguration) {
  return value.name.trim().length > 0 && value.name.length <= 80 && validSourceEndpoint(value.baseUrl)
    && Object.hasOwn(sourceProtocolLabels, value.wireApi) && value.models.length <= 256
    && value.models.every(validSourceModelId)
    && new Set(value.models.map(id => id.toLowerCase())).size === value.models.length;
}

export function sourceModelProtocols(source: OwnerSourcePresentation | undefined, models: string[], model: string) {
  if (!source) return [];
  return [...new Set(effectiveSourceProtocolBindings({ ...source, models })
    .filter(binding => binding.modelIds.some(id => id.toLowerCase() === model.toLowerCase()))
    .map(binding => sourceProtocolLabels[binding.wireApi]))];
}
