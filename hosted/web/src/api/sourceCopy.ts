import { control, record, uuidText } from "./control";
import { confirmDesktopAction } from "./ownerSession";

const states = ["awaiting_approval", "approved", "expired", "committed"] as const;
type State = typeof states[number];
type Proxy = { endpoint: string; authenticated: boolean };
export type CopyPreview = {
  state: State;
  device: string;
  deviceRevision: string;
  operation: string;
  label: string;
  endpoint: string;
  proxy: Proxy | null;
};

export function validSourceCopyCode(code: string) {
  return code.length <= 4096 && /^copy1\.(?:[0-9a-f]{2})+$/.test(code);
}

export async function reviewSourceCopy(code: string, csrf: string): Promise<CopyPreview> {
  if (!validSourceCopyCode(code)) throw new Error("Invalid copy review");
  const v = await control("admin/source-copy", {
    method: "POST",
    headers: { "content-type": "application/json", "x-relay-csrf": csrf },
    body: JSON.stringify({ code }),
  });
  const p = v.plan;
  if (!record(p) || p.origin !== location.origin || typeof p.operation_id !== "string" || !uuidText.test(p.operation_id)
    || typeof v.device_label !== "string" || typeof p.label !== "string" || typeof p.endpoint !== "string"
    || typeof v.device_revision !== "string" || !/^[1-3]$/.test(v.device_revision)
    || (p.proxy !== null && (!record(p.proxy) || typeof p.proxy.endpoint !== "string" || typeof p.proxy.authenticated !== "boolean"))
    || !states.some(state => state === v.state)) throw new Error("Invalid copy result");
  const proxy = p.proxy as Proxy | null;
  return { state: v.state as State, device: v.device_label, deviceRevision: v.device_revision,
    operation: p.operation_id, label: p.label, endpoint: p.endpoint, proxy };
}

export function approveSourceCopy(code: string, reviewed: CopyPreview, csrf: string) {
  return confirmDesktopAction("admin/source-copy", csrf, {
    code, operation_id: reviewed.operation, expected_device_revision: reviewed.deviceRevision,
  });
}
