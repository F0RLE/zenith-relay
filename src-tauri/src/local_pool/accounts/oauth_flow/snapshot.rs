use super::super::oauth::OAuthPendingSession;
use super::{
    OAuthFlowError, OAuthFlowErrorCode, OAuthFlowStatus, PendingSnapshot, AUTHORIZATION_ENDPOINT,
    CALLBACK_PATH, SNAPSHOT_VERSION,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;
use zenith_relay_core::url_has_userinfo;

const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024;

pub(super) fn load_snapshots(root: &Path) -> Result<Vec<PendingSnapshot>, OAuthFlowError> {
    let directory = pending_directory(root);
    ensure_pending_directory(&directory)?;
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|_| snapshot_io())? {
        let entry = entry.map_err(|_| snapshot_io())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("tmp") {
            remove_snapshot(&path).map_err(|_| recovery_required())?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            return Err(recovery_required());
        }
        let login_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(recovery_required)?;
        let login_id = validate_login_id(login_id).map_err(|_| recovery_required())?;
        snapshots.push(read_snapshot(root, &login_id)?);
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.pending.created_at_ms()));
    Ok(snapshots)
}

pub(super) fn read_snapshot(
    root: &Path,
    login_id: &str,
) -> Result<PendingSnapshot, OAuthFlowError> {
    let login_id = validate_login_id(login_id)?;
    let path = snapshot_path(root, &login_id)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            OAuthFlowError::new(
                OAuthFlowErrorCode::RecoveryRequired,
                "OAuth pending snapshot was not found",
            )
            .for_login(&login_id)
        } else {
            snapshot_io().for_login(&login_id)
        }
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_SNAPSHOT_BYTES
    {
        return Err(recovery_required().for_login(&login_id));
    }
    let bytes = fs::read(&path).map_err(|_| snapshot_io().for_login(&login_id))?;
    let snapshot: PendingSnapshot =
        serde_json::from_slice(&bytes).map_err(|_| recovery_required().for_login(&login_id))?;
    validate_snapshot(&snapshot, &login_id)?;
    Ok(snapshot)
}

pub(super) fn validate_snapshot(
    snapshot: &PendingSnapshot,
    expected_login_id: &str,
) -> Result<(), OAuthFlowError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::UnsupportedSnapshotVersion,
            "OAuth pending snapshot version is unsupported",
        )
        .for_login(expected_login_id));
    }
    if snapshot.login_id != expected_login_id
        || snapshot.callback_secret_ref != callback_secret_ref(expected_login_id)
        || !matches!(
            snapshot.status,
            OAuthFlowStatus::Pending | OAuthFlowStatus::CallbackReceived
        )
        || snapshot.pending.created_at_ms() == 0
        || callback_port(&snapshot.pending).is_err()
    {
        return Err(recovery_required().for_login(expected_login_id));
    }
    let authorization_url = Url::parse(&snapshot.authorization_url)
        .map_err(|_| recovery_required().for_login(expected_login_id))?;
    let authorization_endpoint = Url::parse(AUTHORIZATION_ENDPOINT)
        .map_err(|_| recovery_required().for_login(expected_login_id))?;
    if authorization_url.scheme() != authorization_endpoint.scheme()
        || authorization_url.host_str() != authorization_endpoint.host_str()
        || authorization_url.port().is_some()
        || authorization_url.path() != authorization_endpoint.path()
        || url_has_userinfo(&authorization_url)
        || authorization_url.fragment().is_some()
    {
        return Err(recovery_required().for_login(expected_login_id));
    }
    let mut redirect_uri_count = 0;
    for (key, value) in authorization_url.query_pairs() {
        if [
            "code",
            "access_token",
            "refresh_token",
            "id_token",
            "token",
            "client_secret",
            "authorization",
            "password",
            "api_key",
        ]
        .iter()
        .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
        {
            return Err(recovery_required().for_login(expected_login_id));
        }
        if key == "redirect_uri" {
            redirect_uri_count += 1;
            if value != snapshot.pending.redirect_uri() {
                return Err(recovery_required().for_login(expected_login_id));
            }
        }
    }
    if redirect_uri_count != 1 {
        return Err(recovery_required().for_login(expected_login_id));
    }
    if snapshot
        .sign_in_proxy_id
        .as_deref()
        .is_some_and(|proxy_id| !crate::local_pool::accounts::proxy::is_proxy_id(proxy_id))
    {
        return Err(recovery_required().for_login(expected_login_id));
    }
    Ok(())
}

pub(super) fn write_snapshot(
    root: &Path,
    snapshot: &PendingSnapshot,
) -> Result<(), OAuthFlowError> {
    validate_snapshot(snapshot, &snapshot.login_id)?;
    let path = snapshot_path(root, &snapshot.login_id)?;
    ensure_pending_directory(path.parent().ok_or_else(recovery_required)?)?;
    ensure_regular_or_missing(&path)?;
    ensure_regular_or_missing(&path.with_extension("tmp"))?;
    let mut content = serde_json::to_string_pretty(snapshot).map_err(|_| recovery_required())?;
    content.push('\n');
    if content.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(recovery_required().for_login(&snapshot.login_id));
    }
    crate::files::atomic_write(&path, &content)
        .map_err(|_| snapshot_io().for_login(&snapshot.login_id))
}

pub(super) fn ensure_pending_directory(path: &Path) -> Result<(), OAuthFlowError> {
    fs::create_dir_all(path).map_err(|_| snapshot_io())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| snapshot_io())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        Err(recovery_required())
    } else {
        Ok(())
    }
}

fn ensure_regular_or_missing(path: &Path) -> Result<(), OAuthFlowError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(recovery_required()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(snapshot_io()),
    }
}

pub(super) fn remove_snapshot(path: &Path) -> Result<(), ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| ())
        }
        Ok(_) => Err(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

pub(super) fn snapshot_path(root: &Path, login_id: &str) -> Result<PathBuf, OAuthFlowError> {
    let login_id = validate_login_id(login_id)?;
    Ok(pending_directory(root).join(format!("{login_id}.json")))
}

pub(super) fn pending_directory(root: &Path) -> PathBuf {
    root.join("oauth_pending")
}

pub(super) fn callback_secret_ref(login_id: &str) -> String {
    format!("oauth-callback:{login_id}")
}

pub(super) fn validate_login_id(login_id: &str) -> Result<String, OAuthFlowError> {
    let login_id = login_id.trim();
    let uuid = Uuid::parse_str(login_id).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::InvalidLoginId,
            "OAuth login id is invalid",
        )
    })?;
    let canonical = uuid.hyphenated().to_string();
    if login_id != canonical || !login_id.is_ascii() {
        Err(OAuthFlowError::new(
            OAuthFlowErrorCode::InvalidLoginId,
            "OAuth login id is invalid",
        ))
    } else {
        Ok(canonical)
    }
}

pub(super) fn callback_port(pending: &OAuthPendingSession) -> Result<u16, OAuthFlowError> {
    let redirect = Url::parse(pending.redirect_uri()).map_err(|_| recovery_required())?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("localhost")
        || redirect.path() != CALLBACK_PATH
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(recovery_required());
    }
    redirect.port().ok_or_else(recovery_required)
}

fn snapshot_io() -> OAuthFlowError {
    OAuthFlowError::new(
        OAuthFlowErrorCode::SnapshotIo,
        "OAuth pending snapshot could not be accessed",
    )
}

fn recovery_required() -> OAuthFlowError {
    OAuthFlowError::new(
        OAuthFlowErrorCode::RecoveryRequired,
        "OAuth pending state requires recovery",
    )
}
