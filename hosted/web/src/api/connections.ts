import { control, record, uuidText } from "./control";
import { confirmSupplyAction } from "./ownerSession";
import { qualifiedSourceEndpoint, validDeclaredModel } from "./sourceIntake";

export type Serving = {
  revision: string;
  state: "enabled" | "paused" | "trashed";
  in_pool: boolean;
  draining: boolean;
};
export type ServingChange = {
  expected_revision: string;
  operation_id: string;
  action: "pause" | "resume" | "park" | "trash" | "restore" | "remove";
};

export interface SavedSource {
  id: string;
  label: string | null;
  credential_state: "stored" | "revoked";
  proxy_bound: boolean;
  verified_models: string[];
  serving: Serving | null;
  endpoint?: string;
  declared_models?: string[];
}
export interface ConnectionPage { items: SavedSource[]; next_after: string | null }
export type ConnectionView = "api" | "trash";
const invalid = (): never => { throw new Error("Invalid connection inventory"); };
const uuid = (v: unknown): v is string => typeof v === "string" && v.length === 36 && uuidText.test(v);
const model = (v: unknown): v is string => typeof v === "string" && validDeclaredModel(v);

export function parseServing(value: unknown): Serving | null {
  if (value === null) return null;
  if (!record(value) || typeof value.revision !== "string" || !/^(?:0|[1-9]\d{0,18})$/.test(value.revision)
    || value.revision.trim() !== value.revision || BigInt(value.revision) > 9_223_372_036_854_775_807n
    || (value.state !== "enabled" && value.state !== "paused" && value.state !== "trashed")
    || typeof value.in_pool !== "boolean" || typeof value.draining !== "boolean"
    || (value.revision === "0" && value.state !== "enabled") || (value.state === "enabled" && value.draining)
    || (value.state === "trashed" && value.in_pool)) return invalid();
  return { revision: value.revision, state: value.state, in_pool: value.in_pool, draining: value.draining };
}

export function parseConnections(raw: unknown, after?: string, view?: ConnectionView): ConnectionPage {
  if (!record(raw) || !Array.isArray(raw.items) || raw.items.length > 32) return invalid();
  let previous = after ?? "";
  const items = raw.items.map((v: unknown): SavedSource => {
    if (!record(v) || !uuid(v.id) || v.id <= previous
      || (v.label !== null && (typeof v.label !== "string" || !v.label || v.label.trim() !== v.label
        || v.label.length > 96 || /\p{Cc}/u.test(v.label)))
      || (v.credential_state !== "stored" && v.credential_state !== "revoked") || typeof v.proxy_bound !== "boolean"
      || !Array.isArray(v.verified_models) || v.verified_models.length > 128 || !v.verified_models.every(model)
      || v.verified_models.some((m, i, all) => i > 0 && m <= all[i - 1]!)
      || (v.credential_state === "revoked" && v.verified_models.length !== 0)) return invalid();
    const serving = parseServing(v.serving);
    if ((serving === null) !== (v.credential_state === "revoked")) return invalid();
    if (view && (view === "trash") !== (serving?.state === "trashed")) return invalid();
    const intake = v.endpoint !== undefined || v.declared_models !== undefined;
    let spec: Pick<SavedSource, "endpoint" | "declared_models"> = {};
    if (intake) {
      if (typeof v.endpoint !== "string" || qualifiedSourceEndpoint(v.endpoint) !== v.endpoint
        || !Array.isArray(v.declared_models) || v.declared_models.length > 128 || !v.declared_models.every(model)
        || new Set(v.declared_models.map(id => id.toLowerCase())).size !== v.declared_models.length) return invalid();
      spec = { endpoint: v.endpoint, declared_models: v.declared_models };
    }
    previous = v.id;
    return { id: v.id, label: v.label, credential_state: v.credential_state, proxy_bound: v.proxy_bound,
      verified_models: v.verified_models, serving, ...spec };
  });
  const next = raw.next_after;
  if (next !== null && (!uuid(next) || items.length !== 32 || next !== previous)) return invalid();
  return { items, next_after: next };
}

export async function readConnections(view: ConnectionView, after?: string) {
  const query = new URLSearchParams({ view });
  if (after) query.set("after", after);
  return parseConnections(await control(`admin/connections?${query}`), after, view);
}

function servingPath(source: string) {
  if (!uuid(source)) return invalid();
  return `admin/connections/${source}/serving`;
}

export function changeSourceServing(source: string, csrf: string, command: ServingChange) {
  return confirmSupplyAction(servingPath(source), csrf, command);
}

export async function checkSourceServing(source: string, csrf: string, command: ServingChange) {
  const data = await control(servingPath(source), {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(command),
  });
  if (data.applied !== true || data.operation_id !== command.operation_id) return invalid();
}
