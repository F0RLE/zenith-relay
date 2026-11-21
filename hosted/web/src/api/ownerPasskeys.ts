import { control, record, uuidText } from "./control";
import { createPasskey, getAssertion } from "./ownerSession";

const path = "admin/passkeys";
const ceremonies = `${path}/ceremonies`;
export const ownerPasskeyLimit = 8;

export type OwnerPasskey = { id: string; label: string | null; createdMs: number; current: boolean };
type Intent = { action: "add"; label: string | null } | { action: "revoke"; passkey_id: string };
type ProofStep = { ceremonyId: string; publicKey: PublicKeyCredentialRequestOptionsJSON };
export type PasskeyContinuation =
  | { step: "create"; ceremonyId: string; publicKey: PublicKeyCredentialCreationOptionsJSON }
  | ({ step: "confirm" } & ProofStep);
type Result = PasskeyContinuation | { step: "added" | "revoked"; sessionEnded: boolean };

export async function readOwnerPasskeys(signal: AbortSignal): Promise<OwnerPasskey[]> {
  const { passkeys } = await control(path, { signal });
  if (!Array.isArray(passkeys) || !passkeys.length || passkeys.length > ownerPasskeyLimit) throw new Error("Invalid passkey list");
  return passkeys.map((entry: unknown) => {
    if (!record(entry) || typeof entry.id !== "string" || !uuidText.test(entry.id)
      || !(entry.label === null || typeof entry.label === "string") || typeof entry.current !== "boolean"
      || typeof entry.created_ms !== "number" || !Number.isSafeInteger(entry.created_ms)
      || !Number.isFinite(new Date(entry.created_ms).getTime())) throw new Error("Invalid passkey");
    return { id: entry.id, label: entry.label, createdMs: entry.created_ms, current: entry.current };
  });
}

function post(url: string, csrf: string, body: unknown) {
  return control(url, { method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(body) });
}

function options(data: Record<string, unknown>) {
  const { ceremony_id, options } = data;
  if (typeof ceremony_id !== "string" || !uuidText.test(ceremony_id)
    || !record(options) || !record(options.publicKey) || typeof options.publicKey.challenge !== "string") throw new Error("Invalid passkey challenge");
  return { ceremonyId: ceremony_id, publicKey: options.publicKey };
}

function proofOptions(data: Record<string, unknown>): ProofStep {
  const { ceremonyId, publicKey } = options(data);
  if (publicKey.userVerification !== "required") throw new Error("Passkey verification is required");
  return { ceremonyId, publicKey: publicKey as unknown as PublicKeyCredentialRequestOptionsJSON };
}

function result(data: Record<string, unknown>): Result {
  if (data.step === "create") {
    const { ceremonyId, publicKey } = options(data);
    const selection = publicKey.authenticatorSelection;
    if (!record(selection) || selection.userVerification !== "required" || selection.residentKey !== "required")
      throw new Error("Invalid enrollment options");
    return { step: "create", ceremonyId, publicKey: publicKey as unknown as PublicKeyCredentialCreationOptionsJSON };
  }
  if (data.step === "confirm") return { step: "confirm", ...proofOptions(data) };
  if ((data.step === "added" || data.step === "revoked") && typeof data.session_ended === "boolean")
    return { step: data.step, sessionEnded: data.session_ended };
  throw new Error("Invalid passkey result");
}

export async function authorizePasskeyChange(csrf: string, intent: Intent) {
  const data = await post(ceremonies, csrf, intent);
  if (data.step !== "verify") throw new Error("Expected passkey verification");
  const { ceremonyId, publicKey } = proofOptions(data);
  const next = result(await post(`${ceremonies}/${ceremonyId}`, csrf, await getAssertion(publicKey)));
  if (intent.action === "add" ? next.step !== "create" : next.step !== "revoked") throw new Error("Unexpected passkey result");
  return next;
}

export async function continuePasskeyChange(csrf: string, step: PasskeyContinuation) {
  const proof = step.step === "create" ? await createPasskey(step.publicKey) : await getAssertion(step.publicKey);
  const next = result(await post(`${ceremonies}/${step.ceremonyId}`, csrf, proof));
  if (step.step === "create" ? next.step !== "confirm" : next.step !== "added") throw new Error("Unexpected passkey result");
  return next;
}
