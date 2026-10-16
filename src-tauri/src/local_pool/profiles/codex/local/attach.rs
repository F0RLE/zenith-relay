use super::*;
use std::fs;

pub(in crate::local_pool::profiles::codex) fn attach_local_locked(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: LocalAttachOptions<'_>,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let key_id = key_id.trim();
    let local_key = local_key.trim();
    let base_url = base_url.trim_end_matches('/');
    let catalog_json = options.catalog_json;
    let provider_id = options.provider_id;
    let supports_websockets = options.supports_websockets;
    let bound_oauth = normalize_bound_oauth(options.bound_oauth)?;
    if key_id.is_empty() || local_key.is_empty() || base_url.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "profile credential ID, base URL, and credential are required",
        ));
    }
    fs::create_dir_all(codex_home).map_err(io_error)?;
    fs::create_dir_all(backup_root).map_err(io_error)?;
    let _ = local_backup(codex_home, backup_root)?;
    let catalog_path = managed_model_catalog_path(backup_root)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let auth_path = codex_home.join(AUTH_FILE);
    let backup_path = backup_path(backup_root);
    let original_config_bytes = read_optional_bytes(&config_path)?;
    let original_auth_bytes = read_optional_bytes(&auth_path)?;
    let original_backup_bytes = read_optional_bytes(&backup_path)?;
    let original_catalog_bytes = read_optional_bytes(&catalog_path)?;
    let original_config = snapshot_text(&original_config_bytes, &config_path)?.unwrap_or_default();
    let original_auth = snapshot_text(&original_auth_bytes, &auth_path)?;
    let mut document = parse_config(original_config)?;
    validate_config_shape(&document)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "ChatGPT profile is already attached to an OAuth account",
        ));
    }
    let existing_backup = parse_backup_snapshot(&original_backup_bytes, &backup_path)?;
    if existing_backup.as_ref().is_some_and(|backup| {
        !valid_managed_model_catalog(backup, &catalog_path, &original_catalog_bytes)
    }) {
        return Err(profile_restore_blocked());
    }
    let had_managed_catalog = existing_backup
        .as_ref()
        .and_then(|backup| backup.managed_model_catalog_path.as_deref())
        .is_some();
    let orphaned_managed_provider = existing_backup.is_none() && document_has_provider(&document);

    if catalog_json.is_some()
        && !had_managed_catalog
        && original_catalog_bytes.is_some()
        && !orphaned_managed_provider
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT model catalog exists without a profile backup",
        ));
    }

    let external_takeover = existing_backup
        .as_ref()
        .is_some_and(|backup| external_provider_took_over(&document, backup));
    if existing_backup.is_some()
        && !managed_config_matches(&document, existing_backup.as_ref().unwrap())
        && !external_takeover
    {
        return Err(profile_restore_blocked());
    }
    if let Some(backup) = existing_backup.as_ref() {
        if !external_takeover
            && !managed_auth_matches_snapshot(&original_auth_bytes, &auth_path, backup)?
            && !previous_auth_matches_snapshot(&original_auth_bytes, backup)
        {
            return Err(profile_restore_blocked());
        }
    }

    let user_catalog_path = if external_takeover {
        existing_backup
            .as_ref()
            .and_then(|backup| external_model_catalog(&document, backup))
    } else {
        existing_backup
            .as_ref()
            .and_then(|backup| {
                backup
                    .managed_model_catalog_path
                    .as_ref()
                    .and(backup.previous_model_catalog_json.as_ref())
                    .cloned()
            })
            .or_else(|| root_model_catalog_json(&document))
    };
    let catalog = catalog_json
        .map(|content| {
            catalog::build_managed_model_catalog(
                codex_home,
                user_catalog_path.as_deref(),
                had_managed_catalog
                    .then_some(original_catalog_bytes.as_deref())
                    .flatten(),
                content,
            )
        })
        .transpose()?;
    let managed_model_reasoning_effort = reasoning_effort_for_attach(&document, catalog.as_deref());

    let staged = stage_managed_profile(
        existing_backup,
        provider_id,
        supports_websockets,
        orphaned_managed_provider,
        &mut document,
        &original_auth_bytes,
        &managed_model_reasoning_effort,
        external_takeover,
        secrets,
        original_auth,
        key_id,
        local_key,
        base_url,
        bound_oauth,
        &catalog,
        &catalog_path,
    )?;
    commit_staged_profile(
        staged,
        external_takeover,
        original_config,
        original_auth,
        secrets,
        &original_config_bytes,
        &config_path,
        &backup_path,
        &original_backup_bytes,
        &catalog_path,
        &original_catalog_bytes,
        &catalog,
        had_managed_catalog,
        codex_home,
        &auth_path,
        &original_auth_bytes,
    )
}

struct StagedLocalAttach {
    created_backup: bool,
    backup: ProfileBackup,
    rebased_secret: Option<(String, Option<String>)>,
    managed_config: String,
    managed_auth: String,
}

#[allow(clippy::too_many_arguments)]
fn stage_managed_profile(
    existing_backup: Option<ProfileBackup>,
    provider_id: &str,
    supports_websockets: bool,
    orphaned_managed_provider: bool,
    document: &mut DocumentMut,
    original_auth_bytes: &Option<Vec<u8>>,
    managed_model_reasoning_effort: &Option<String>,
    external_takeover: bool,
    secrets: &impl SecretBackend,
    original_auth: Option<&str>,
    key_id: &str,
    local_key: &str,
    base_url: &str,
    bound_oauth: Option<BoundOAuthProfile<'_>>,
    catalog: &Option<String>,
    catalog_path: &Path,
) -> Result<StagedLocalAttach> {
    let created_backup = existing_backup.is_none();
    let mut backup = existing_backup.unwrap_or(ProfileBackup {
        version: 1,
        managed_provider_id: provider_id.to_owned(),
        projection_secret_ref: None,
        previous_model_provider: (!orphaned_managed_provider)
            .then(|| root_model_provider(document))
            .flatten(),
        previous_model_catalog_json: (!orphaned_managed_provider)
            .then(|| root_model_catalog_json(document))
            .flatten(),
        previous_model: (!orphaned_managed_provider)
            .then(|| root_model(document))
            .flatten(),
        previous_review_model: (!orphaned_managed_provider)
            .then(|| root_review_model(document))
            .flatten(),
        previous_chatgpt_base_url: (!orphaned_managed_provider)
            .then(|| root_chatgpt_base_url(document))
            .flatten(),
        previous_openai_base_url: (!orphaned_managed_provider)
            .then(|| root_openai_base_url(document))
            .flatten(),
        previous_model_reasoning_effort: root_model_reasoning_effort(document),
        previous_auth_hash: original_auth_bytes.as_deref().map(bytes_hash),
        previous_auth_secret_ref: None,
        managed_key_id: String::new(),
        managed_key_hash: String::new(),
        managed_base_url: String::new(),
        bound_oauth_account_id: None,
        managed_oauth_access_hash: None,
        managed_bearer_in_config: false,
        managed_supports_websockets: Some(supports_websockets),
        managed_model_reasoning_effort_cleared: false,
        managed_model_reasoning_effort: None,
        managed_show_ultra_picker: false,
        previous_show_ultra_picker: None,
        managed_model_catalog_path: None,
        managed_model_catalog_hash: None,
        managed_model_catalog_pending_hash: None,
        managed_model_catalog_pending_remove: false,
        attach_pending: false,
        restore_pending: false,
    });
    if !created_backup
        && backup.managed_model_catalog_path.is_none()
        && backup.managed_model_catalog_hash.is_none()
    {
        backup.previous_model_catalog_json = root_model_catalog_json(document);
    }
    if !created_backup {
        if let Some(secret_ref) = backup.projection_secret_ref.as_deref() {
            let config_before = projection::config_before(secret_ref, secrets)?
                .as_deref()
                .map(parse_config)
                .transpose()?;
            if let Some(config_before) = config_before {
                if backup.previous_model.is_none() {
                    backup.previous_model = root_model(&config_before);
                }
                if backup.previous_review_model.is_none() {
                    backup.previous_review_model = root_review_model(&config_before);
                }
                if backup.previous_chatgpt_base_url.is_none() {
                    backup.previous_chatgpt_base_url = root_chatgpt_base_url(&config_before);
                }
                if backup.previous_openai_base_url.is_none() {
                    backup.previous_openai_base_url = root_openai_base_url(&config_before);
                }
            }
        }
    }
    if created_backup || external_takeover || !backup.managed_model_reasoning_effort_cleared {
        backup.previous_model_reasoning_effort = root_model_reasoning_effort(document);
    }
    backup.managed_model_reasoning_effort_cleared = true;
    backup.managed_model_reasoning_effort = managed_model_reasoning_effort.clone();
    if !backup.managed_show_ultra_picker
        && desktop_bool(document, DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY) != Some(true)
    {
        backup.previous_show_ultra_picker =
            desktop_bool(document, DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY);
        backup.managed_show_ultra_picker = true;
    }
    let rebased_secret = if external_takeover {
        backup.previous_model_provider = root_model_provider(document);
        backup.previous_model_catalog_json = external_model_catalog(document, &backup);
        backup.previous_model = root_model(document);
        backup.previous_review_model = root_review_model(document);
        backup.previous_chatgpt_base_url = root_chatgpt_base_url(document);
        backup.previous_openai_base_url = root_openai_base_url(document);
        backup.previous_auth_hash = original_auth_bytes.as_deref().map(bytes_hash);
        let secret_ref = backup
            .previous_auth_secret_ref
            .clone()
            .unwrap_or_else(|| BACKUP_SECRET_REF.to_string());
        let previous_secret = secrets.load(&secret_ref)?;
        if let Some(previous_auth) = zenith_relay_core::omit_blank(original_auth) {
            secrets.save(&secret_ref, previous_auth)?;
            backup.previous_auth_secret_ref = Some(secret_ref.clone());
        } else {
            secrets.delete(&secret_ref)?;
            backup.previous_auth_secret_ref = None;
        }
        Some((secret_ref, previous_secret))
    } else {
        None
    };
    if created_backup {
        if let Some(previous_auth) = zenith_relay_core::omit_blank(original_auth) {
            secrets.save(BACKUP_SECRET_REF, previous_auth)?;
            backup.previous_auth_secret_ref = Some(BACKUP_SECRET_REF.to_string());
        }
    } else if backup.previous_auth_hash.is_none() {
        if let Some(secret_ref) = backup.previous_auth_secret_ref.as_deref() {
            backup.previous_auth_hash = secrets
                .load(secret_ref)?
                .map(|content| bytes_hash(content.as_bytes()));
        }
    }
    backup.managed_key_id = key_id.to_string();
    backup.managed_key_hash = key_hash(local_key);
    backup.managed_base_url = base_url.to_string();
    let managed_oauth_access_hash = bound_oauth
        .as_ref()
        .filter(|oauth| oauth.tokens.id_token().is_some())
        .map(|oauth| key_hash(oauth.tokens.access_token()));
    let project_bound_oauth = managed_oauth_access_hash.is_some();
    backup.bound_oauth_account_id = bound_oauth
        .as_ref()
        .map(|oauth| oauth.account_id.to_string());
    backup.managed_oauth_access_hash = managed_oauth_access_hash;
    backup.managed_bearer_in_config = true;
    backup.managed_supports_websockets = Some(supports_websockets);
    let previous_managed_catalog_path = backup.managed_model_catalog_path.clone();
    let previous_managed_catalog_hash = backup.managed_model_catalog_hash.clone();
    backup.managed_model_catalog_path = if catalog.is_some() {
        Some(portable_path_string(catalog_path))
    } else {
        previous_managed_catalog_path
    };
    backup.managed_model_catalog_hash = previous_managed_catalog_hash;
    backup.managed_model_catalog_pending_hash = catalog.as_deref().map(key_hash);
    backup.managed_model_catalog_pending_remove =
        catalog.is_none() && backup.managed_model_catalog_path.is_some();
    backup.attach_pending = true;
    backup.restore_pending = false;
    attach_config(
        document,
        base_url,
        local_key,
        catalog
            .as_ref()
            .map(|_| portable_path_string(catalog_path))
            .as_deref(),
        managed_model_reasoning_effort.as_deref(),
        supports_websockets,
    );
    if provider_id != PROVIDER_ID {
        let providers = document["model_providers"]
            .as_table_mut()
            .expect("attach creates providers");
        let mut provider = providers
            .remove(PROVIDER_ID)
            .expect("attach creates provider");
        provider["name"] = value(READY_API_PROVIDER_NAME);
        providers.insert(provider_id, provider);
        document["model_provider"] = value(provider_id);
    }
    backup.managed_provider_id = provider_id.to_owned();
    let managed_config = document.to_string();
    let credential = match bound_oauth.as_ref() {
        Some(oauth) if project_bound_oauth => {
            account_auth_content(oauth.tokens, oauth.provider_account_id)?
        }
        _ => auth_content(local_key),
    };
    let managed_auth = projection::merge_auth(original_auth, Some(&credential))?
        .expect("an attached credential is present");
    Ok(StagedLocalAttach {
        created_backup,
        backup,
        rebased_secret,
        managed_config,
        managed_auth,
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_staged_profile(
    staged: StagedLocalAttach,
    external_takeover: bool,
    original_config: &str,
    original_auth: Option<&str>,
    secrets: &impl SecretBackend,
    original_config_bytes: &Option<Vec<u8>>,
    config_path: &Path,
    backup_path: &Path,
    original_backup_bytes: &Option<Vec<u8>>,
    catalog_path: &Path,
    original_catalog_bytes: &Option<Vec<u8>>,
    catalog: &Option<String>,
    had_managed_catalog: bool,
    codex_home: &Path,
    auth_path: &Path,
    original_auth_bytes: &Option<Vec<u8>>,
) -> Result<()> {
    let StagedLocalAttach {
        created_backup,
        mut backup,
        rebased_secret,
        managed_config,
        managed_auth,
    } = staged;
    if created_backup || external_takeover {
        let baseline = if external_takeover {
            let mut baseline = parse_config(original_config)?;
            remove_relay_provider_tables(&mut baseline);
            if !RELAY_PROVIDER_IDS.contains(&backup.managed_provider_id.as_str()) {
                remove_managed_provider(&mut baseline, &backup.managed_provider_id);
            }
            restore_root_string(
                &mut baseline,
                "model_catalog_json",
                backup.previous_model_catalog_json.as_deref(),
            );
            Some(baseline.to_string())
        } else {
            snapshot_text(original_config_bytes, config_path)?.map(str::to_owned)
        };
        backup.projection_secret_ref = Some(
            match projection::save(baseline.as_deref(), &managed_config, original_auth, secrets) {
                Ok(secret_ref) => secret_ref,
                Err(error) => {
                    return Err(with_rollback(
                        error,
                        rollback_uncommitted_secrets(
                            created_backup,
                            &backup,
                            secrets,
                            &rebased_secret,
                        ),
                    ));
                }
            },
        );
    }
    if let Some(secret_ref) = backup.projection_secret_ref.clone() {
        if let Err(error) = projection::record_show_ultra_picker(&secret_ref, secrets) {
            return Err(with_rollback(
                error,
                rollback_uncommitted_secrets(created_backup, &backup, secrets, &rebased_secret),
            ));
        }
    }
    let backup_content = match serialize_backup(&backup) {
        Ok(content) => content,
        Err(error) => {
            return Err(with_rollback(
                error,
                rollback_uncommitted_secrets(created_backup, &backup, secrets, &rebased_secret),
            ));
        }
    };
    if let Err(error) = replace_if_unchanged(backup_path, original_backup_bytes, &backup_content) {
        return Err(with_rollback(
            error,
            rollback_uncommitted_secrets(created_backup, &backup, secrets, &rebased_secret),
        ));
    }

    if let Err(error) = apply_model_catalog_change(
        catalog_path,
        original_catalog_bytes,
        catalog.as_deref(),
        had_managed_catalog,
    ) {
        return Err(with_rollback(
            error,
            merge_rollbacks(
                rollback_backup(
                    created_backup,
                    backup_path,
                    &backup_content,
                    original_backup_bytes,
                    &backup,
                    secrets,
                ),
                restore_secret_snapshot(&rebased_secret, secrets),
            ),
        ));
    }

    if let Err(error) = replace_if_unchanged(config_path, original_config_bytes, &managed_config) {
        return Err(with_rollback(
            error,
            merge_rollbacks(
                rollback_model_catalog_change(
                    catalog_path,
                    catalog.as_deref(),
                    had_managed_catalog,
                    original_catalog_bytes,
                ),
                merge_rollbacks(
                    rollback_backup(
                        created_backup,
                        backup_path,
                        &backup_content,
                        original_backup_bytes,
                        &backup,
                        secrets,
                    ),
                    restore_secret_snapshot(&rebased_secret, secrets),
                ),
            ),
        ));
    }
    if let Err(error) = replace_if_unchanged(auth_path, original_auth_bytes, &managed_auth) {
        let config_rollback = rollback_file(config_path, &managed_config, original_config_bytes);
        let backup_rollback = merge_rollbacks(
            rollback_model_catalog_change(
                catalog_path,
                catalog.as_deref(),
                had_managed_catalog,
                original_catalog_bytes,
            ),
            merge_rollbacks(
                rollback_backup(
                    created_backup,
                    backup_path,
                    &backup_content,
                    original_backup_bytes,
                    &backup,
                    secrets,
                ),
                restore_secret_snapshot(&rebased_secret, secrets),
            ),
        );
        return Err(with_rollback(
            error,
            merge_rollbacks(config_rollback, backup_rollback),
        ));
    }
    let pending_backup_bytes = backup_content.as_bytes().to_vec();
    let mut committed_backup = backup.clone();
    committed_backup.managed_model_catalog_path =
        catalog.as_ref().map(|_| portable_path_string(catalog_path));
    committed_backup.managed_model_catalog_hash = catalog.as_deref().map(key_hash);
    committed_backup.managed_model_catalog_pending_hash = None;
    committed_backup.managed_model_catalog_pending_remove = false;
    committed_backup.attach_pending = false;
    committed_backup.restore_pending = false;
    let committed_backup_content = serialize_backup(&committed_backup)?;
    replace_if_unchanged(
        backup_path,
        &Some(pending_backup_bytes),
        &committed_backup_content,
    )?;
    // Invalidate only after the managed catalog and config have been built
    // and committed. The cache is also an input to catalog construction, so
    // removing it before that point would discard Codex's native capability
    // template during a Relay switch.
    let _ = invalidate_models_cache(codex_home);
    Ok(())
}

fn rollback_uncommitted_secrets(
    created_backup: bool,
    backup: &ProfileBackup,
    secrets: &impl SecretBackend,
    rebased_secret: &Option<(String, Option<String>)>,
) -> Result<()> {
    merge_rollbacks(
        cleanup_created_backup_secret(created_backup, backup, secrets),
        restore_secret_snapshot(rebased_secret, secrets),
    )
}

fn normalize_bound_oauth(
    bound_oauth: Option<BoundOAuthProfile<'_>>,
) -> Result<Option<BoundOAuthProfile<'_>>> {
    let Some(bound_oauth) = bound_oauth else {
        return Ok(None);
    };
    let account_id = bound_oauth.account_id.trim();
    let provider_account_id = bound_oauth.provider_account_id.trim();
    if account_id.is_empty()
        || account_id.chars().any(char::is_control)
        || provider_account_id.is_empty()
        || bound_oauth.tokens.refresh_token().is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth binding requires active refresh and account tokens",
        ));
    }
    Ok(Some(BoundOAuthProfile {
        account_id,
        tokens: bound_oauth.tokens,
        provider_account_id,
    }))
}
