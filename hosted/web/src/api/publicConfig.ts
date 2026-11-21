import { uuidText } from "./control";
const endpoint = "/api/hosted/v1/public/config";

export const policyKinds = ["terms", "privacy", "refunds", "support"] as const;
export type PolicyKind = typeof policyKinds[number];
type PolicyLinks = Partial<Record<PolicyKind, string>>;
export type ClosedSite = { displayName: string; policyLinks?: PolicyLinks; loginProvider?: string };
const object = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function readPolicyLinks(value: unknown): PolicyLinks {
  if (!object(value) || Object.keys(value).some(k => !policyKinds.some(kind => kind === k))) throw new Error("Invalid policy links");
  const links: PolicyLinks = {};
  for (const key of policyKinds) {
    if (!Object.hasOwn(value, key)) continue;
    const raw = value[key];
    if (typeof raw !== "string" || raw.length > 2048 || !raw.startsWith("https://") || /[\\?#\s\p{Cc}]/u.test(raw)) throw new Error("Invalid policy URL");
    const url = new URL(raw);
    if (url.protocol !== "https:" || url.username || url.password || url.href !== raw) throw new Error("Invalid policy URL");
    links[key] = url.href;
  }
  return links;
}

export function readPublicConfig(value: unknown): ClosedSite {
  if (object(value) && object(value.meta) && value.meta.api_version === "1"
    && typeof value.meta.request_id === "string" && /^[\da-f]{8}(-[\da-f]{4}){3}-[\da-f]{12}$/i.test(value.meta.request_id)
    && object(value.data)) {
    const { display_name, login_methods, registration_open, studio_enabled } = value.data;
    if (typeof display_name === "string" && display_name.trim() === display_name
      && Array.from(display_name).length > 0 && Array.from(display_name).length <= 64
      && !/\p{Cc}/u.test(display_name) && registration_open === false && studio_enabled === false
      && Array.isArray(login_methods) && login_methods.length <= 1
      && login_methods.every(id => typeof id === "string" && uuidText.test(id))) {
      return { displayName: display_name, ...(login_methods.length ? { loginProvider: login_methods[0] as string } : {}),
        ...(value.data.policy_links === undefined ? {} : { policyLinks: readPolicyLinks(value.data.policy_links) }) };
    }
  }
  throw new Error("Incompatible public config");
}

export async function fetchPublicConfig(signal: AbortSignal): Promise<ClosedSite> {
  const response = await fetch(endpoint, {
    method: "GET", mode: "same-origin", credentials: "omit", redirect: "error", cache: "no-store",
    headers: { Accept: "application/json" }, signal,
  });
  if (!response.ok || response.headers.get("content-type")?.split(";")[0]?.trim() !== "application/json") {
    throw new Error("Public config unavailable");
  }
  return readPublicConfig(await response.json());
}
