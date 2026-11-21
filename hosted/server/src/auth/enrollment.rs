use webauthn_rs::prelude::*;
use webauthn_rs_proto::ResidentKeyRequirement;

pub(crate) fn options(
    auth: &Webauthn,
    user: Uuid,
    exclude: Option<Vec<CredentialID>>,
) -> Option<(CreationChallengeResponse, PasskeyRegistration)> {
    let (mut options, state) = auth
        .start_passkey_registration(user, "owner", "Owner", exclude)
        .ok()?;
    let selection = options.public_key.authenticator_selection.as_mut()?;
    selection.resident_key = Some(ResidentKeyRequirement::Required);
    selection.require_resident_key = true;
    Some((options, state))
}

pub(crate) fn register(
    auth: &Webauthn,
    state: &PasskeyRegistration,
    reg: &RegisterPublicKeyCredential,
) -> Option<(RequestChallengeResponse, PasskeyAuthentication, Passkey)> {
    if reg.type_ != "public-key" {
        return None;
    }
    let key = auth.finish_passkey_registration(reg, state).ok()?;
    if key.cred_id() != &reg.raw_id {
        return None;
    }
    let (mut options, proof) = auth
        .start_passkey_authentication(std::slice::from_ref(&key))
        .ok()?;
    // Prove that the new key works without an allow-list, as it will at sign-in.
    options.public_key.allow_credentials.clear();
    Some((options, proof, key))
}

pub(crate) fn prove(
    auth: &Webauthn,
    user: Uuid,
    state: &PasskeyAuthentication,
    mut key: Passkey,
    assertion: &PublicKeyCredential,
) -> Option<Passkey> {
    if assertion.type_ != "public-key" || assertion.get_user_unique_id() != Some(user.as_bytes()) {
        return None;
    }
    let proof = auth.finish_passkey_authentication(assertion, state).ok()?;
    if !proof.user_verified() {
        return None;
    }
    key.update_credential(&proof)?;
    Some(key)
}
