import { control, record, uuidText } from "./control";

export type BrowserSession = {
  id: string;
  current: boolean;
  createdAt: number;
  lastSeenAt: number;
};

function epochSeconds(v: unknown): v is number {
  return typeof v === "number" && Number.isInteger(v) && v > 0 && v <= 8_640_000_000_000;
}

export function parseCustomerSessions(data: Record<string, unknown>): BrowserSession[] {
  if (!Array.isArray(data.sessions) || data.sessions.length < 1 || data.sessions.length > 8)
    throw new Error("Invalid session list");
  const ids = new Set<string>();
  return data.sessions.map((row: unknown, index) => {
    if (!record(row) || typeof row.id !== "string" || !uuidText.test(row.id) || ids.has(row.id)
      || row.current !== (index === 0) || !epochSeconds(row.created_at_s) || !epochSeconds(row.last_seen_at_s))
      throw new Error("Invalid browser session");
    ids.add(row.id);
    return { id: row.id, current: row.current, createdAt: row.created_at_s * 1000, lastSeenAt: row.last_seen_at_s * 1000 };
  });
}

export async function readCustomerSessions(signal: AbortSignal) {
  return parseCustomerSessions(await control("account/sessions", { signal }));
}

export async function endCustomerSession(csrf: string, id: string) {
  if (!uuidText.test(id)) throw new Error("Invalid session ID");
  const data = await control(`account/sessions/${id}`, { method: "DELETE", headers: { "X-Relay-CSRF": csrf } });
  if (data.id !== id || data.revoked !== true || typeof data.signed_out !== "boolean")
    throw new Error("Session revocation not acknowledged");
  return data.signed_out;
}
