use super::super::origin::PinnedOrigin;
use super::{RemoteClientError, RemoteProfileCredential};
use zenith_relay_core::protocol::{ProfileKeyRotation, PROFILE_KEY_ROTATION_SCHEMA_VERSION};

pub(super) fn validate_profile_credential(
    origin: &PinnedOrigin,
    credential: RemoteProfileCredential,
) -> Result<RemoteProfileCredential, RemoteClientError> {
    validate_profile_credential_fields(
        origin,
        &credential.key_id,
        &credential.base_url,
        &credential.secret,
    )?;
    Ok(credential)
}

pub(super) fn validate_profile_key_rotation(
    origin: &PinnedOrigin,
    rotation: ProfileKeyRotation,
) -> Result<ProfileKeyRotation, RemoteClientError> {
    if rotation.schema_version != PROFILE_KEY_ROTATION_SCHEMA_VERSION
        || remote_object_path("profile/credential/rotations", &rotation.rotation_id).is_err()
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    validate_profile_credential_fields(
        origin,
        &rotation.key_id,
        &rotation.base_url,
        &rotation.secret,
    )?;
    Ok(rotation)
}

fn validate_profile_credential_fields(
    origin: &PinnedOrigin,
    key_id: &str,
    base_url: &str,
    secret: &str,
) -> Result<(), RemoteClientError> {
    let expected_base_url = origin.endpoint("/v1")?;
    let actual_base_url =
        url::Url::parse(base_url).map_err(|_| RemoteClientError::InvalidResponse)?;
    if actual_base_url != expected_base_url
        || !zenith_relay_core::is_ascii_token(key_id, 128)
        || !secret.starts_with("zrs_")
        || secret.len() < 24
        || secret.len() > 256
        || secret.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RemoteClientError::InvalidResponse);
    }
    Ok(())
}

pub(super) fn remote_object_path(
    collection: &str,
    object_id: &str,
) -> Result<String, RemoteClientError> {
    if !zenith_relay_core::is_ascii_token(object_id, 128) {
        return Err(RemoteClientError::InvalidResponse);
    }
    Ok(format!("/{collection}/{object_id}"))
}
