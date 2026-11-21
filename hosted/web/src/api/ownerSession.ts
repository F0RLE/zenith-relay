import { ControlError, control as request, record, uuidText } from "./control";
export { ControlError as OwnerApiError } from "./control";

export class OwnerPasskeyError extends Error {
  constructor() { super("Owner confirmation failed"); }
}

function mode(v: unknown): v is "setup" | "maintenance" | "qualification" {
  return v === "setup" || v === "maintenance" || v === "qualification";
}

export async function readOwnerOverview(signal: AbortSignal) {
  const data = await request("admin/overview", { signal });
  if (!mode(data.service_mode) || typeof data.csrf_token !== "string" || !/^[a-f0-9]{64}$/.test(data.csrf_token)
    || [data.web_api_intake, data.web_model_checks].some(value => value !== undefined && typeof value !== "boolean")) {
    throw new Error("Invalid owner overview");
  }
  return { mode: data.service_mode, csrf: data.csrf_token, webApiIntake: data.web_api_intake === true,
    webModelChecks: data.web_model_checks === true } as const;
}

export async function readService() {
  const data = await request("admin/operations/service");
  if (!mode(data.mode) || typeof data.revision !== "string" || !/^[1-9]\d{0,18}$/.test(data.revision))
    throw new Error("Invalid service state");
  return { mode: data.mode, revision: data.revision };
}

export async function changeCustomerAccess(csrf: string, operation: string, revision: string, ids: string[], pause: boolean) {
  await confirmOwnerAction("admin/operations/service/transitions", csrf, {
    operation_id: operation, expected_revision: revision, candidate_ids: ids,
    target: pause ? "maintenance" : "qualification", reason_code: pause ? "pause_customer_access" : "verify_customer_access",
  });
}

export function confirmPaymentAction<T extends { operation_id: string }>(path: string, csrf: string, command: T) {
  return confirmOwnerAction(path, csrf, command, "admin/payments/confirmations");
}

export function confirmSupplyAction<T extends { operation_id: string }>(path: string, csrf: string, command: T) {
  return confirmOwnerAction(path, csrf, command, "admin/supply/confirmations");
}

export function confirmDesktopAction<T extends { operation_id: string }>(path: string, csrf: string, command: T) {
  return confirmOwnerAction(path, csrf, command, "admin/desktop/confirmations");
}

export async function confirmOwnerAction<T extends { operation_id: string }>(path: string, csrf: string, command: T, verify = path) {
  await confirmWithPasskey(path, csrf, command, verify, data =>
    data.applied === true && data.operation_id === command.operation_id ? true : undefined);
}

export async function confirmWithPasskey<R>(path: string, csrf: string, command: object, verify: string,
  receipt: (data: Record<string, unknown>) => R | undefined) {
  const post = (route: string, body: unknown) => request(route, {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf }, body: JSON.stringify(body),
  });
  const start = await post(`${path}/options`, command);
  const prior = receipt(start);
  if (prior !== undefined) return prior;
  if (typeof start.ceremony_id !== "string" || !uuidText.test(start.ceremony_id) || !record(start.options))
    throw new Error("Invalid confirmation options");
  const proof = await getAssertion(assertionOptions(start.options.publicKey)).catch(() => { throw new OwnerPasskeyError(); });
  try {
    const acknowledged = receipt(await post(`${verify}/${start.ceremony_id}/verify`, proof));
    if (acknowledged === undefined) throw new Error("Operation not acknowledged");
    return acknowledged;
  } catch (error) {
    if (error instanceof ControlError && (error.code === "invalid_credential"
      || error.code === "passkey_ceremony_expired" || error.code === "passkey_changed")) throw new OwnerPasskeyError();
    throw error;
  }
}

function assertionOptions(pk: unknown): PublicKeyCredentialRequestOptionsJSON {
  if (!record(pk) || typeof pk.challenge !== "string" || pk.userVerification !== "required")
    throw new Error("Invalid passkey options");
  return { ...pk, challenge: pk.challenge, userVerification: pk.userVerification };
}

export function supportsOwnerPasskey() {
  return window.isSecureContext && typeof PublicKeyCredential !== "undefined"
    && typeof PublicKeyCredential.parseRequestOptionsFromJSON === "function";
}

export async function getAssertion(options: PublicKeyCredentialRequestOptionsJSON) {
  const key = await navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options) });
  if (!(key instanceof PublicKeyCredential)) throw new Error("No passkey assertion");
  return key.toJSON();
}

export async function createPasskey(options: PublicKeyCredentialCreationOptionsJSON) {
  const key = await navigator.credentials.create({ publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options) });
  if (!(key instanceof PublicKeyCredential)) throw new Error("No passkey");
  return key.toJSON();
}

export async function signInOwner() {
  const options = await request("auth/admin/sign-in/options", { method: "POST" });
  const proof = await getAssertion(assertionOptions(options.publicKey));
  const data = await request("auth/admin/sign-in/verify", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(proof),
  });
  if (data.signed_in !== true) throw new Error("Sign-in not confirmed");
}

export async function signOutOwner(csrf: string) {
  const data = await request("auth/admin/logout", { method: "POST", headers: { "X-Relay-CSRF": csrf } });
  if (data.signed_out !== true) throw new Error("Sign-out not confirmed");
}
