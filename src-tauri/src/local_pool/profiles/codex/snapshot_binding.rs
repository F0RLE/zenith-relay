//! A saved managed connection includes its identity and encrypted undo data.
//! Secret references are regenerated on restore; snapshots remain reusable.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Binding {
    Account { backup: AccountProfileBackup },
    Gateway { backup: ProfileBackup },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::local_pool::profiles) struct ManagedBindingSnapshot {
    binding: Binding,
    previous_auth: Option<String>,
    projection: Option<String>,
}

impl ManagedBindingSnapshot {
    pub(super) fn capture(
        home: &Path,
        root: &Path,
        snapshot: &UserProfileSnapshot,
        secrets: &impl SecretBackend,
    ) -> Result<Option<Self>> {
        ensure_single_profile_backup(home, root)?;
        let document = parse_config(snapshot.config.as_deref().unwrap_or_default())?;
        let auth = snapshot.auth.as_ref().map(|auth| auth.as_bytes().to_vec());
        let auth_path = home.join(AUTH_FILE);
        let binding = if let Some(path) = account_backup_for_profile(home, root)? {
            let backup = read_account_backup(&path)?;
            if !snapshot::account_connection_matches(&document)
                || !account_auth_matches_snapshot(&auth, &auth_path, &backup.managed_access_hash)?
            {
                return Ok(None);
            }
            Binding::Account { backup }
        } else if let Some(backup) = local_backup(home, root)? {
            if !managed_config_matches(&document, &backup)
                || !managed_auth_matches_snapshot(&auth, &auth_path, &backup)?
            {
                return Ok(None);
            }
            if backup.attach_pending || backup.restore_pending {
                return Err(profile_restore_blocked());
            }
            Binding::Gateway { backup }
        } else {
            return Ok(None);
        };
        let (auth_ref, projection_ref) = match &binding {
            Binding::Account { backup } => (
                &backup.previous_auth_secret_ref,
                &backup.projection_secret_ref,
            ),
            Binding::Gateway { backup } => (
                &backup.previous_auth_secret_ref,
                &backup.projection_secret_ref,
            ),
        };
        let load = |reference: &Option<String>| {
            reference
                .as_ref()
                .map(|reference| {
                    secrets.load(reference)?.ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::RecoveryRequired,
                            "Snapshot binding undo data is missing",
                        )
                    })
                })
                .transpose()
        };
        Ok(Some(Self {
            previous_auth: load(auth_ref)?,
            projection: load(projection_ref)?,
            binding,
        }))
    }

    pub(super) fn install(
        &self,
        home: &Path,
        root: &Path,
        restored: &UserProfileSnapshot,
        secrets: &impl SecretBackend,
    ) -> Result<()> {
        let document = parse_config(restored.config.as_deref().unwrap_or_default())?;
        let auth = restored.auth.as_ref().map(|auth| auth.as_bytes().to_vec());
        let auth_path = home.join(AUTH_FILE);
        let save = |payload: &Option<String>| {
            payload
                .as_ref()
                .map(|payload| {
                    let reference = format!("profile:codex:snapshot-undo:{}", uuid::Uuid::new_v4());
                    secrets.save(&reference, payload)?;
                    Ok(reference)
                })
                .transpose()
        };
        match &self.binding {
            Binding::Account { backup } => {
                if backup.version != 1
                    || portable_path_value(&backup.profile_dir) != portable_path_string(home)
                    || !snapshot::account_connection_matches(&document)
                    || !account_auth_matches_snapshot(
                        &auth,
                        &auth_path,
                        &backup.managed_access_hash,
                    )?
                {
                    return Err(profile_restore_blocked());
                }
                let mut backup = backup.clone();
                backup.profile_dir = home.to_string_lossy().into_owned();
                backup.previous_auth_secret_ref = save(&self.previous_auth)?;
                backup.projection_secret_ref = save(&self.projection)?;
                replace_if_unchanged(
                    &account_backup_path(root, home),
                    &None,
                    &serialize_account_backup(&backup)?,
                )
            }
            Binding::Gateway { backup } => {
                if backup.version != 1
                    || backup.attach_pending
                    || backup.restore_pending
                    || !managed_config_matches(&document, backup)
                    || !managed_auth_matches_snapshot(&auth, &auth_path, backup)?
                {
                    return Err(profile_restore_blocked());
                }
                let mut backup = backup.clone();
                backup.previous_auth_secret_ref = save(&self.previous_auth)?;
                backup.projection_secret_ref = save(&self.projection)?;
                let catalog_path = managed_model_catalog_path(root)?;
                if backup.managed_model_catalog_path.is_some() {
                    let bytes = read_optional_bytes(&catalog_path)?;
                    backup.managed_model_catalog_hash = bytes.as_deref().map(bytes_hash);
                    backup.managed_model_catalog_pending_hash = None;
                    backup.managed_model_catalog_pending_remove = false;
                }
                replace_if_unchanged(&backup_path(root), &None, &serialize_backup(&backup)?)
            }
        }
    }
}
