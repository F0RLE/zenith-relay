import { control, record, uuidText } from "./control";
import { confirmSupplyAction } from "./ownerSession";
import { validDeclaredModel } from "./sourceIntake";

export const checkLimits = {
  max_input_bytes: [14, 60000],
  max_output_tokens: [1, 1000000],
  tokens_per_byte: [1, 16],
  overhead_tokens: [0, 65536],
} as const;
type Limit = keyof typeof checkLimits;
export type CheckInputs = Record<Limit, number> & { model: string };
export type CheckCommand = { operation_id: string; expected_revision: string; inputs: CheckInputs };

const states = ["queued", "running", "verified", "not_verified", "unconfirmed", "expired"] as const;
type State = typeof states[number];
export type CheckReceipt = { operation: string; model: string; state: State };
export const checkPending = (result: CheckReceipt | null) => result?.state === "queued" || result?.state === "running";

const path = (source: string) => {
  if (!uuidText.test(source)) throw new Error("Invalid connection ID");
  return `admin/connections/${source}/checks`;
};

export function approveModelCheck(source: string, csrf: string, command: CheckCommand) {
  return confirmSupplyAction(path(source), csrf, command);
}

export async function readModelCheck(source: string, operation?: string, signal?: AbortSignal): Promise<CheckReceipt | null> {
  if (operation && !uuidText.test(operation)) throw new Error("Invalid check operation");
  const { check } = await control(path(source) + (operation ? `?operation=${operation}` : ""), signal ? { signal } : {});
  if (check === null) return null;
  if (!record(check) || typeof check.operation_id !== "string" || !uuidText.test(check.operation_id)
    || operation && check.operation_id !== operation || typeof check.model !== "string" || !validDeclaredModel(check.model)
    || !states.includes(check.state as State)) throw new Error("Invalid model check receipt");
  return { operation: check.operation_id, model: check.model, state: check.state as State };
}
