import { control, csrfText, record, uuidText } from "./control";

export async function beginCustomer(id: string) {
  if (!uuidText.test(id)) throw new Error("Invalid login method");
  const data = await control(`auth/customer/${id}/begin`, { method: "POST" });
  if (typeof data.authorization_url !== "string") throw new Error("Invalid authorization URL");
  const u = new URL(data.authorization_url);
  if (u.username || u.password || u.hash || (u.protocol !== "https:" && !(location.protocol === "http:"
    && u.protocol === "http:" && (/^127(?:\.\d{1,3}){3}$/.test(u.hostname) || u.hostname === "[::1]")))) throw new Error("Invalid authorization origin");
  location.assign(u.href);
}

export async function readCustomer(signal: AbortSignal) {
  const data = await control("account", { signal });
  const amount = (v: unknown) => {
    if (typeof v !== "string" || !/^(0|[1-9]\d{0,18})$/.test(v) || BigInt(v) > 9223372036854775807n)
      throw new Error("Invalid wallet amount");
    return BigInt(v);
  };
  if (typeof data.workspace_id !== "string" || !uuidText.test(data.workspace_id) || typeof data.csrf_token !== "string"
    || !csrfText.test(data.csrf_token) || !record(data.wallet) || data.wallet.currency !== "USD" || data.wallet.precision !== 6)
    throw new Error("Invalid customer account");
  const reserved = amount(data.wallet.reserved_micro_usd), inference = amount(data.wallet.inference_reserved_micro_usd);
  const refund = amount(data.wallet.refund_reserved_micro_usd);
  if (inference + refund !== reserved) throw new Error("Invalid reserve breakdown");
  return { workspaceId: data.workspace_id, csrf: data.csrf_token, available: amount(data.wallet.available_micro_usd),
    reserved, inference, refund };
}

export async function logoutCustomer(csrf: string) {
  const data = await control("auth/customer/logout", { method: "POST", headers: { "X-Relay-CSRF": csrf } });
  if (data.signed_out !== true) throw new Error("Logout not acknowledged");
}
