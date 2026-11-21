import { control, record, uuidText } from "./control";
import { confirmSupplyAction } from "./ownerSession";
import { validDeclaredModel } from "./sourceIntake";

type ModelFlag = { model: string; allowed: boolean };

export type MemberModels = {
  scope_id: string;
  source_revision: string;
  revision: string;
  items: ModelFlag[];
};

export type MemberModelsChange = {
  operation_id: string;
  scope_id: string;
  expected_source_revision: string;
  expected_revision: string;
  changes: ModelFlag[];
};

const invalid = (): never => { throw new Error("Invalid pool member models"); };
const uuid = (v: unknown): v is string => typeof v === "string" && v.length === 36
  && uuidText.test(v) && v !== "00000000-0000-0000-0000-000000000000";
const revision = (v: unknown): v is string => typeof v === "string" && /^(?:0|[1-9]\d{0,18})$/.test(v)
  && v.trim() === v && BigInt(v) <= 9_223_372_036_854_775_807n;

export function parseMemberModels(data: Record<string, unknown>): MemberModels {
  const { scope_id, source_revision, revision: version, items } = data;
  if (!uuid(scope_id) || !revision(source_revision) || !revision(version)
    || !Array.isArray(items) || items.length === 0 || items.length > 128) return invalid();
  let previous = "";
  const flags = items.map((item: unknown) => {
    if (!record(item) || typeof item.model !== "string" || !validDeclaredModel(item.model)
      || item.model <= previous || typeof item.allowed !== "boolean") return invalid();
    previous = item.model;
    return { model: item.model, allowed: item.allowed };
  });
  return { scope_id, source_revision, revision: version, items: flags };
}

function path(source: string) {
  if (!uuid(source)) return invalid();
  return `admin/pools/members/${source}/models`;
}

export async function readMemberModels(source: string, signal: AbortSignal) {
  return parseMemberModels(await control(path(source), { signal }));
}

export function changeMemberModels(source: string, csrf: string, command: MemberModelsChange) {
  return confirmSupplyAction(path(source), csrf, command);
}
