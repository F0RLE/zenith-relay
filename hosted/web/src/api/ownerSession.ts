import { control as request, record } from "./control";
export { ControlError as OwnerApiError } from "./control";

function mode(v: unknown): v is "setup" | "maintenance" | "qualification" {
  return v === "setup" || v === "maintenance" || v === "qualification";
}

export async function readOwnerOverview(signal: AbortSignal) {
  const data = await request("admin/overview", { signal });
  if (!mode(data.service_mode) || typeof data.csrf_token !== "string" || !/^[a-f0-9]{64}$/.test(data.csrf_token)) {
    throw new Error("Invalid owner overview");
  }
  return { mode: data.service_mode, csrf: data.csrf_token } as const;
}

export async function readService() {
  const data = await request("admin/operations/service");
  if (!mode(data.mode) || typeof data.revision !== "string" || !/^[1-9]\d{0,18}$/.test(data.revision))
    throw new Error("Invalid service state");
  return { mode: data.mode, revision: data.revision };
}

export async function changeCustomerAccess(csrf: string, operation: string, revision: string, ids: string[], pause: boolean) {
  const data = await request("admin/operations/service/transitions", {
    method: "POST", headers: { "Content-Type": "application/json", "X-Relay-CSRF": csrf },
    body: JSON.stringify({ operation_id: operation, expected_revision: revision, candidate_ids: ids,
      target: pause ? "maintenance" : "qualification", reason_code: pause ? "pause_customer_access" : "verify_customer_access" }),
  });
  if (data.operation_id !== operation || data.applied !== true) throw new Error("Transition not acknowledged");
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

export async function signInOwner() {
  const options = await request("auth/admin/sign-in/options", { method: "POST" });
  const pk = options.publicKey;
  if (!record(pk) || typeof pk.challenge !== "string" || pk.userVerification !== "required") throw new Error("Invalid passkey options");
  const proof = await getAssertion({ ...pk, challenge: pk.challenge, userVerification: pk.userVerification });
  const data = await request("auth/admin/sign-in/verify", {
    method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(proof),
  });
  if (data.signed_in !== true) throw new Error("Sign-in not confirmed");
}

export async function signOutOwner(csrf: string) {
  const data = await request("auth/admin/logout", { method: "POST", headers: { "X-Relay-CSRF": csrf } });
  if (data.signed_out !== true) throw new Error("Sign-out not confirmed");
}
