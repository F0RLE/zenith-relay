import { control, record, uuidText } from "./control";

export interface PairedDevice {
  id: string; label: string; state: "pending" | "active" | "revoked" | "expired";
  revision: string; expires_at_s: number; last_used_at_s: number | null;
}
export interface DeviceList { installation_id: string; items: PairedDevice[] }
const fail = (): never => { throw new Error("Invalid device response"); };
const id = (s: unknown): s is string => typeof s === "string" && s.length === 36 && uuidText.test(s) && s !== "00000000-0000-0000-0000-000000000000";
const stamp = (n: unknown): n is number => typeof n === "number" && Number.isSafeInteger(n) && n > 0 && n <= 253402300799;
export const validDeviceLabel = (s: string) =>
  !!s && s === s.trim() && new TextEncoder().encode(s).length <= 64 && !/\p{Cc}/u.test(s);
export const validDeviceApproval = (device: string, s: string) =>
  id(device) && s.length === 109 && s.startsWith(`review1.${device}.`) && /^[0-9a-f]{64}$/.test(s.slice(-64));
export function parseDevices(data: unknown, origin: string): DeviceList {
  if (!record(data) || data.origin !== origin || !id(data.installation_id) || !Array.isArray(data.items) || data.items.length > 32) return fail();
  const seen = new Set<string>();
  const items = data.items.map((v: unknown): PairedDevice => {
    if (!record(v) || !id(v.id) || seen.has(v.id) || v.scope !== "connections.read" || typeof v.label !== "string" ||
      !validDeviceLabel(v.label) || !stamp(v.expires_at_s) ||
      (v.last_used_at_s !== null && !stamp(v.last_used_at_s)) || typeof v.revision !== "string" || !["1","2","3"].includes(v.revision) ||
      (v.state !== "pending" && v.state !== "active" && v.state !== "revoked" && v.state !== "expired") ||
      (v.state === "pending" && v.revision !== "1") || (v.state === "active" && v.revision !== "2") ||
      (v.state === "revoked" && v.revision === "1") || (v.state === "expired" && v.revision === "3") ||
      (v.revision === "1" && v.last_used_at_s !== null)) return fail();
    seen.add(v.id);
    return { id:v.id,label:v.label,state:v.state,revision:v.revision,expires_at_s:v.expires_at_s,last_used_at_s:v.last_used_at_s };
  });
  return { installation_id:data.installation_id, items };
}
export const readDevices = async () => parseDevices(await control("admin/devices"),location.origin);
export interface ChangeDevice {
  action: "invite"|"approve"|"revoke"; operation_id:string; device_id:string; expected_revision:string;
  label?:string; approval_code?:string;
}
export async function changeDevice(change: ChangeDevice, csrf:string) {
  const result = await control("admin/devices",{method:"POST",headers:{"Content-Type":"application/json","X-Relay-CSRF":csrf},body:JSON.stringify(change)});
  if (result.applied !== true || result.operation_id !== change.operation_id) return fail();
}
