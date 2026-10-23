use super::super::{ImportItemError, ItemResult};
use crate::local_pool::commands::sync_records_or_rollback;
use crate::local_pool::models::{LocalGatewayKeyRecord, ProviderSourceRecord};
use crate::local_pool::state::DesktopState;
use crate::local_pool::store::secret_store;
use zenith_relay_core::error_codes;

pub(crate) async fn persist_imported_source(
    state: &DesktopState,
    source_record: &ProviderSourceRecord,
    api_key: &str,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<()> {
    crate::diagnostics::breadcrumb(
        "source-import",
        "persist_started",
        &[("in_pool", source_record.in_pool.to_string())],
    );
    let (old_sources, old_keys) = current_source_records(state)?;
    let old_secret = existing
        .map(|source| {
            secret_store::load(&source.secret_ref).map_err(|_| {
                ImportItemError::new(
                    error_codes::SOURCE_SECRET_STORE_FAILED,
                    "source secret store is unavailable",
                )
            })
        })
        .transpose()?
        .flatten();
    state
        .store()
        .and_then(|mut store| store.invalidate_source_refresh(&source_record.id))
        .map_err(|_| {
            ImportItemError::new(
                error_codes::SOURCE_STORE_FAILED,
                "source revision could not be saved",
            )
        })?;
    secret_store::save(&source_record.secret_ref, api_key).map_err(|_| {
        ImportItemError::new(
            error_codes::SOURCE_SECRET_STORE_FAILED,
            "failed to save source credentials",
        )
    })?;
    if state
        .store()
        .map_err(|_| {
            ImportItemError::new(
                error_codes::SOURCE_STORE_FAILED,
                "source store is unavailable",
            )
        })?
        .upsert_source(source_record.clone())
        .is_err()
    {
        restore_source_secret(&source_record.secret_ref, old_secret.as_deref())?;
        return Err(ImportItemError::new(
            error_codes::SOURCE_STORE_FAILED,
            "failed to save source record",
        ));
    }
    let runtime_sync_required =
        source_record.in_pool || existing.is_some_and(|source| source.in_pool);
    if runtime_sync_required {
        crate::diagnostics::breadcrumb("source-import", "runtime_sync_started", &[]);
        if sync_records_or_rollback(state, old_sources, old_keys)
            .await
            .is_err()
        {
            let store = state.store().map_err(|_| {
                ImportItemError::new(
                    error_codes::SOURCE_STORE_FAILED,
                    "source store is unavailable",
                )
            })?;
            let rolled_back = match existing {
                Some(previous_source) => store.source(&source_record.id) == Some(previous_source),
                None => store.source(&source_record.id).is_none(),
            };
            drop(store);
            if rolled_back {
                restore_source_secret(&source_record.secret_ref, old_secret.as_deref())?;
            }
            return Err(ImportItemError::new(
                error_codes::GATEWAY_SYNC_FAILED,
                "failed to apply source to the local gateway",
            ));
        }
        crate::diagnostics::breadcrumb("source-import", "runtime_sync_completed", &[]);
    } else {
        crate::diagnostics::breadcrumb("source-import", "runtime_sync_skipped", &[]);
    }
    Ok(())
}

pub(crate) fn current_source_records(
    state: &DesktopState,
) -> ItemResult<(Vec<ProviderSourceRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state.store().map_err(|_| {
        ImportItemError::new(
            error_codes::SOURCE_STORE_FAILED,
            "source store is unavailable",
        )
    })?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

pub(crate) fn restore_source_secret(
    secret_ref: &str,
    previous_secret: Option<&str>,
) -> ItemResult<()> {
    match previous_secret {
        Some(secret) => secret_store::save(secret_ref, secret),
        None => secret_store::delete(secret_ref),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous source credentials"))
}
