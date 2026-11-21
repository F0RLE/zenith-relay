import { control, uuidText } from "./control";
import { confirmWithPasskey } from "./ownerSession";

export function qualifiedSourceEndpoint(raw: string): string | null {
  const value = raw.trim().replace(/\/$/, "");
  if (!/^https?:\/\//i.test(value) || /[?#@\\\s]/u.test(value)) return null;
  try {
    const url = new URL(value);
    if (url.pathname !== "/v1" || url.href.length > 256) return null;
    const hostname = url.hostname;
    const domain = hostname.includes(".") && hostname.split(".").every(part =>
      /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(part));
    if (url.protocol === "https:" && !url.port && domain && !/^[\d.]+$/.test(hostname)) return url.href;
    if (url.protocol === "http:" && url.port && url.port !== "0" &&
      (hostname === "[::1]" || /^127(?:\.\d{1,3}){3}$/.test(hostname))) return url.href;
  } catch { /* Keep the input unsubmitted. */ }
  return null;
}

export const validDeclaredModel = (value: string) =>
  /^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$/.test(value) && value.trim() === value;

type SourceReceipt = { state: "saved"; source: string } | { state: "pending" | "not_saved" };
const hex = (bytes: Uint8Array) => Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");

function receipt(data: Record<string, unknown>, operation: string): SourceReceipt {
  if (data.operation_id === operation) {
    if (data.state === "pending" || data.state === "not_saved") return { state: data.state };
    if (data.state === "saved" && typeof data.source_id === "string" && uuidText.test(data.source_id))
      return { state: data.state, source: data.source_id };
  }
  throw new Error("Invalid source save receipt");
}

export async function readSourceReceipt(operation: string) {
  if (!uuidText.test(operation)) throw new Error("Invalid source operation");
  return receipt(await control(`admin/connections/intake/${operation}`), operation);
}

export async function saveApiSource(csrf: string, operation: string,
  draft: { label: string; endpoint: string; declared_models: string[] }, key: string, proxy: string | undefined,
  onSending: () => void) {
  const verifier = crypto.getRandomValues(new Uint8Array(32));
  const body = new TextEncoder().encode(`${key}\n${proxy ?? ""}`);
  try {
    const signer = await crypto.subtle.importKey("raw", verifier, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    const [digest, mac] = await Promise.all([
      crypto.subtle.digest("SHA-256", verifier), crypto.subtle.sign("HMAC", signer, body),
    ]);
    await confirmWithPasskey("admin/connections/intake", csrf, {
      ...draft, operation_id: operation, proxied: proxy !== undefined,
      verifier_digest: hex(new Uint8Array(digest)), payload_mac: hex(new Uint8Array(mac)),
    }, "admin/supply/confirmations", data => data.state === "approved" && data.operation_id === operation ? true : undefined);
    onSending();
    const result = receipt(await control(`transfer/intake/${operation}`, {
      method: "POST", credentials: "omit", headers: {
        "Content-Type": "application/octet-stream", Authorization: `Bearer ${hex(verifier)}`,
      }, body,
    }), operation);
    if (result.state !== "saved") throw new Error("Source save not acknowledged");
    return result.source;
  } finally {
    verifier.fill(0);
    body.fill(0);
  }
}
