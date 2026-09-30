use super::*;

impl<B: SecretBackend> ImportSessionStore<B> {
    pub fn new(root: PathBuf, secrets: B) -> Self {
        Self { root, secrets }
    }

    pub fn start(
        &self,
        content: &str,
        source_file: Option<&str>,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = Uuid::new_v4().hyphenated().to_string();
        self.start_with_id(&session_id, content, source_file, existing_identity_keys)
    }

    pub fn resume(
        &self,
        session_id: &str,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let snapshot = match read_snapshot(&self.root, &session_id, true) {
            Ok(snapshot) => snapshot,
            Err(error) if error.code == ImportSessionErrorCode::SessionNotFound => {
                read_snapshot(&self.root, &session_id, false)?
            }
            Err(error) => return Err(error),
        };
        let content = self.load_secret(&snapshot.secret_ref, &session_id)?;
        let (base, stable_source_file) =
            parse_stable(&content, snapshot.source_file.as_deref(), &[])?;
        let reparsed_preview = preview_value(&base.preview)?;
        if reparsed_preview != snapshot.preview {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotMismatch,
                "import session snapshot does not match its secret",
            )
            .for_session(&session_id));
        }
        let mut parsed = if existing_identity_keys.is_empty() {
            base
        } else {
            parse_import(
                &content,
                stable_source_file.as_deref(),
                existing_identity_keys,
            )
            .map_err(ImportSessionError::from_import)?
        };
        let prepared = snapshot.final_preview.is_some();
        if let Some(final_preview) = snapshot.final_preview {
            let final_preview: ImportPreview =
                serde_json::from_value(final_preview).map_err(|_| {
                    ImportSessionError::new(
                        ImportSessionErrorCode::SnapshotInvalid,
                        "prepared import preview is invalid",
                    )
                    .for_session(&session_id)
                })?;
            if selectable_row_count(&final_preview) != parsed.items.len() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotMismatch,
                    "prepared import preview does not match its secret",
                )
                .for_session(&session_id));
            }
            for (item, row) in parsed
                .items
                .iter_mut()
                .zip(final_preview.rows.iter().filter(|row| row.selectable))
            {
                item.item_id = row.item_id.clone();
            }
            parsed.preview = final_preview;
        }
        Ok(session_from_parsed(
            session_id,
            snapshot.created_at_ms,
            parsed,
            prepared,
        ))
    }

    pub fn prepare(
        &self,
        session_id: &str,
        content: Option<&str>,
        final_preview: ImportPreview,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let original = read_snapshot(&self.root, &session_id, false)?;
        self.clear_prepared(&session_id)?;
        let original_content = if content.is_none() {
            Some(self.load_secret(&original.secret_ref, &session_id)?)
        } else {
            None
        };
        let content = content.or(original_content.as_deref()).ok_or_else(|| {
            ImportSessionError::new(
                ImportSessionErrorCode::SnapshotMismatch,
                "prepared import content is missing",
            )
            .for_session(&session_id)
        })?;
        let (base, stable_source_file) =
            parse_stable(content, original.source_file.as_deref(), &[])?;
        if base.items.len() != selectable_row_count(&final_preview) {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotMismatch,
                "prepared import preview does not match prepared credentials",
            )
            .for_session(&session_id));
        }
        let preview = preview_value(&base.preview)?;
        let final_preview = preview_value(&final_preview)?;
        validate_preview(&final_preview)?;
        let stores_prepared_secret = original_content.is_none();
        let secret_ref = if stores_prepared_secret {
            prepared_secret_ref(&session_id)
        } else {
            original.secret_ref.clone()
        };
        let snapshot = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.clone(),
            created_at_ms: original.created_at_ms,
            source_file: stable_source_file,
            secret_ref: secret_ref.clone(),
            preview,
            final_preview: Some(final_preview),
        };
        if stores_prepared_secret {
            self.secrets.save(&secret_ref, content).map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "failed to save prepared import credentials",
                )
                .for_session(&session_id)
            })?;
        }
        let path = prepared_snapshot_path(&self.root, &session_id)?;
        if let Err(error) = write_snapshot_new(&path, &snapshot) {
            if stores_prepared_secret && self.secrets.delete(&secret_ref).is_err() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::RecoveryRequired,
                    "failed to roll back prepared import credentials",
                )
                .for_session(&session_id));
            }
            return Err(error.for_session(&session_id));
        }
        self.resume(&session_id, existing_identity_keys)
    }

    pub fn cancel(&self, session_id: &str) -> Result<(), ImportSessionError> {
        self.clear(session_id)
    }

    pub fn complete(&self, session_id: &str) -> Result<(), ImportSessionError> {
        self.clear(session_id)
    }

    pub fn cleanup_expired(&self) -> Result<usize, ImportSessionError> {
        self.cleanup_expired_at(now_ms())
    }

    pub(in crate::local_pool::accounts::import_session) fn cleanup_expired_at(
        &self,
        now_ms: u64,
    ) -> Result<usize, ImportSessionError> {
        let directory = self.root.join("imports");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotIo,
                    "failed to inspect import session directory",
                ))
            }
        };
        let mut stale = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotIo,
                    "failed to inspect import session directory",
                )
            })?;
            let file_type = entry.file_type().map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotIo,
                    "failed to inspect import session snapshot",
                )
            })?;
            if !file_type.is_file() || file_type.is_symlink() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(session_id) = name.strip_suffix(".json") else {
                continue;
            };
            if session_id.ends_with(".prepared") || validate_session_id(session_id).is_err() {
                continue;
            }
            let Ok(snapshot) = read_snapshot(&self.root, session_id, false) else {
                continue;
            };
            if snapshot.created_at_ms.saturating_add(IMPORT_SESSION_TTL_MS) <= now_ms {
                stale.push(session_id.to_string());
            }
        }
        for session_id in &stale {
            self.clear(session_id)?;
        }
        Ok(stale.len())
    }

    pub(in crate::local_pool::accounts::import_session) fn start_with_id(
        &self,
        session_id: &str,
        content: &str,
        source_file: Option<&str>,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let (base, stable_source_file) = parse_stable(content, source_file, &[])?;
        let preview = preview_value(&base.preview)?;
        validate_preview(&preview)?;
        let created_at_ms = now_ms();
        let secret_ref = secret_ref(&session_id);
        let snapshot = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.clone(),
            created_at_ms,
            source_file: stable_source_file.clone(),
            secret_ref: secret_ref.clone(),
            preview,
            final_preview: None,
        };
        let parsed = if existing_identity_keys.is_empty() {
            base
        } else {
            parse_import(
                content,
                stable_source_file.as_deref(),
                existing_identity_keys,
            )
            .map_err(ImportSessionError::from_import)?
        };
        let path = snapshot_path(&self.root, &session_id)?;
        if path.exists() {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SessionCollision,
                "import session already exists",
            )
            .for_session(&session_id));
        }
        if self
            .secrets
            .load(&secret_ref)
            .map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "import session secret store is unavailable",
                )
                .for_session(&session_id)
            })?
            .is_some()
        {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SessionCollision,
                "import session already exists",
            )
            .for_session(&session_id));
        }
        self.secrets.save(&secret_ref, content).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to save import session secret",
            )
            .for_session(&session_id)
        })?;
        if let Err(error) = write_snapshot_new(&path, &snapshot) {
            if self.secrets.delete(&secret_ref).is_err() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::RecoveryRequired,
                    "failed to roll back import session secret",
                )
                .for_session(&session_id));
            }
            return Err(error.for_session(&session_id));
        }

        Ok(session_from_parsed(
            session_id,
            created_at_ms,
            parsed,
            false,
        ))
    }

    fn load_secret(
        &self,
        secret_ref: &str,
        session_id: &str,
    ) -> Result<String, ImportSessionError> {
        self.secrets
            .load(secret_ref)
            .map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "import session secret is unavailable",
                )
                .for_session(session_id)
            })?
            .ok_or_else(|| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretMissing,
                    "import session secret is missing",
                )
                .for_session(session_id)
            })
    }

    fn clear(&self, session_id: &str) -> Result<(), ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        self.clear_prepared(&session_id)?;
        let secret_ref = secret_ref(&session_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to delete import session secret",
            )
            .for_session(&session_id)
        })?;
        let path = snapshot_path(&self.root, &session_id)?;
        remove_snapshot_file(&path).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "import session secret was cleared but snapshot cleanup is incomplete",
            )
            .for_session(&session_id)
        })?;
        let temp = snapshot_temp_path(&path);
        remove_snapshot_file(&temp).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "import session secret was cleared but temporary snapshot cleanup is incomplete",
            )
            .for_session(&session_id)
        })?;
        Ok(())
    }

    fn clear_prepared(&self, session_id: &str) -> Result<(), ImportSessionError> {
        let secret_ref = prepared_secret_ref(session_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to delete prepared import credentials",
            )
            .for_session(session_id)
        })?;
        let path = prepared_snapshot_path(&self.root, session_id)?;
        remove_snapshot_file(&path).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "prepared import credentials were cleared but snapshot cleanup is incomplete",
            )
            .for_session(session_id)
        })?;
        remove_snapshot_file(&snapshot_temp_path(&path)).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "prepared import temporary snapshot cleanup is incomplete",
            )
            .for_session(session_id)
        })
    }
}
