import type { ProxyPoolEntry } from "../../../../src/src/features/relay/api/types/proxy";

// Safe summaries only: proxy authentication never belongs to a browser snapshot.
export type OwnerProxy = Pick<ProxyPoolEntry, "id" | "label" | "endpoint" | "assignedAccountIds" | "createdAtMs" | "lastCheck"> & { hasAuthentication?: boolean };

const protocols = new Set(["http:", "https:", "socks5:", "socks5h:"]);

function parseProxy(value: string): { endpoint: string; hasAuthentication: boolean; identity: string } | null {
  let input = value.trim();
  if (!input || /\s/.test(input)) return null;
  if (!input.includes("://")) {
    if (!input.includes("@")) {
      const match = /^(\[[^\]]+\]|[^:/@]+):(\d+)(?::([^:]+):(.+))?$/.exec(input);
      if (!match) return null;
      input = match[3] ? `${encodeURIComponent(match[3])}:${encodeURIComponent(match[4]!)}@${match[1]}:${match[2]}` : input;
    }
    input = `http://${input}`;
  }
  try {
    const url = new URL(input);
    if (!protocols.has(url.protocol) || !url.hostname || url.port === "0" || url.pathname && url.pathname !== "/" || url.search || url.hash) return null;
    return { endpoint: `${url.protocol}//${url.host}`, hasAuthentication: Boolean(url.username || url.password), identity: url.href };
  } catch { return null; }
}

export function safeProxyEndpoint(value: string): string | null {
  return parseProxy(value)?.endpoint ?? null;
}

export function parseProxyImport(content: string, existing: readonly OwnerProxy[] = []) {
  const lines = content.split(/\r?\n/).map((value, index) => ({ value: value.trim(), line: index + 1 })).filter(item => item.value);
  const invalidLines: number[] = [];
  const entries: Pick<OwnerProxy, "endpoint" | "hasAuthentication">[] = [];
  const identities = new Set<string>();
  let duplicates = 0;
  if (lines.length > 200) return { entries, invalidLines, duplicates, tooMany: true };
  for (const { value, line } of lines) {
    const parsed = parseProxy(value);
    if (!parsed) { invalidLines.push(line); continue; }
    if (identities.has(parsed.identity) || !parsed.hasAuthentication && existing.some(item => !item.hasAuthentication && safeProxyEndpoint(item.endpoint) === parsed.endpoint)) { duplicates++; continue; }
    identities.add(parsed.identity);
    entries.push({ endpoint: parsed.endpoint, hasAuthentication: parsed.hasAuthentication });
  }
  return { entries, invalidLines, duplicates, tooMany: false };
}

export function assignProxyAccounts(pool: readonly OwnerProxy[], proxyId: string, ids: readonly string[], allowed: readonly string[]): OwnerProxy[] {
  if (!pool.some(proxy => proxy.id === proxyId)) return [...pool];
  const selected = new Set(ids.filter(id => allowed.includes(id)));
  return pool.map(proxy => ({ ...proxy, assignedAccountIds: proxy.id === proxyId ? [...new Set([...proxy.assignedAccountIds.filter(id => !allowed.includes(id)), ...selected])] : proxy.assignedAccountIds.filter(id => !selected.has(id)) }));
}
