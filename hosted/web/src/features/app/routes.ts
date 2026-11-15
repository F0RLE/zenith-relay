export const customerPages = [
  { id: "overview", name: "nav.overview" },
  { id: "usage", name: "nav.usage" },
  { id: "models", name: "nav.models" },
  { id: "status", name: "common.status" },
  { id: "keys", name: "nav.keys" },
  { id: "billing", name: "nav.balance" },
] as const;

export const ownerPages = [
  { id: "overview", name: "nav.overview", group: "" },
  { id: "connections", name: "nav.connections", group: "nav.resources" },
  { id: "pools", name: "nav.pool", group: "nav.resources" },
  { id: "models", name: "nav.models", group: "nav.resources" },
  { id: "usage", name: "nav.usage", group: "nav.events" },
  { id: "notifications", name: "shell.notifications", group: "nav.events" },
] as const;

export type CustomerPage = (typeof customerPages)[number]["id"];
export type OwnerPage = (typeof ownerPages)[number]["id"];
export type HostedPage = CustomerPage | OwnerPage;
export type HostedArea = "customer" | "owner";
export interface HostedRoute { area: HostedArea; page: HostedPage; found: boolean }

export function readHostedRoute(url: URL): HostedRoute {
  const legacy = url.pathname === "/preview.html";
  const area = url.pathname.startsWith("/admin") ? "owner" : "customer";
  const pages = area === "owner" ? ownerPages : customerPages;
  const parts = url.pathname.split("/").filter(Boolean);
  const candidate = legacy ? url.searchParams.get("page") ?? "overview" : parts[1] ?? "overview";
  const found = (legacy || parts[0] === (area === "owner" ? "admin" : "app")) && parts.length <= 2 && pages.some(page => page.id === candidate);
  return { area, page: found ? candidate as HostedPage : "overview", found };
}

export function hostedPageUrl(area: HostedArea, page: HostedPage, current: URL): URL {
  const url = new URL(current);
  url.hash = "";
  url.search = "";
  if (current.pathname === "/preview.html" && area === "customer") url.searchParams.set("page", page);
  else url.pathname = `/${area === "owner" ? "admin" : "app"}/${page}`;
  return url;
}
