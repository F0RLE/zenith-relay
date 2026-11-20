use super::{
    DesktopState, accounts::proxy::ProxyPool, models::ProviderSourceRecord, store::secret_store,
};
use crate::storage_paths::StoragePaths;
use serde::Serialize;
use uuid::Uuid;
use zenith_relay_core::WireApi;
use zenith_relay_hosted_device::{CopyInput, CopyKey, CopyPlan, CopyProxy, Destination};
use zeroize::Zeroizing;

const SLOT: &str = "hosted:copy:v1";
const STORAGE: &str = "device_storage_unavailable";
const UNAVAILABLE: &str = "copy_source_unavailable";

#[derive(Serialize)]
pub(crate) struct SourceChoice {
    id: Uuid,
    label: String,
}

fn id(source: &ProviderSourceRecord) -> Option<Uuid> {
    let raw = source.id.strip_prefix("source_")?;
    Uuid::parse_str(raw)
        .ok()
        .filter(|u| !u.is_nil() && u.simple().to_string() == raw)
}

fn plan(
    source: &ProviderSourceRecord,
    target: &Destination,
    operation_id: Uuid,
    proxy: Option<CopyProxy>,
) -> Option<CopyPlan> {
    let bindings = source.effective_protocol_bindings().ok()?;
    // A physical Responses endpoint can also have local client-side adapters.
    // Copy the native route, not an adapter that only looks like Responses.
    if !bindings
        .iter()
        .any(|b| b.wire_api == WireApi::Responses && b.adapter.is_passthrough())
    {
        return None;
    }
    let result = CopyPlan {
        origin: target.origin().to_owned(),
        installation_id: target.installation(),
        device_id: target.device(),
        operation_id,
        source_id: id(source)?,
        label: source.name.clone(),
        endpoint: source
            .base_url
            .strip_suffix('/')
            .unwrap_or(&source.base_url)
            .to_owned(),
        proxy,
    };
    result.valid().then_some(result)
}

pub(crate) fn choices(
    state: &DesktopState,
    target: &Destination,
) -> Result<Vec<SourceChoice>, &'static str> {
    let store = state.store().map_err(|_| STORAGE)?;
    Ok(store
        .sources()
        .iter()
        .filter_map(|source| {
            // The list is intentionally metadata only; the selected source resolves its actual route.
            let p = plan(source, target, target.device(), None)?;
            Some(SourceChoice {
                id: p.source_id,
                label: p.label,
            })
        })
        .collect())
}

/// Called under the local setup guard, with no network or mutation.
pub(crate) fn snapshot(
    state: &DesktopState,
    target: &Destination,
    operation: Uuid,
    source_id: Uuid,
) -> Result<CopyInput, &'static str> {
    let source = state
        .store()
        .map_err(|_| STORAGE)?
        .source(&format!("source_{}", source_id.simple()))
        .cloned()
        .ok_or(UNAVAILABLE)?;
    let root = StoragePaths::from_root(&state.root).vault_root();
    // API sources use the source resolver, not the account/common-proxy selection policy.
    let proxy = if let Some(id) = source.proxy_id.as_deref() {
        ProxyPool::load_read_only(&root)
            .and_then(|pool| pool.source_url(Some(id)))
            .map_err(|_| UNAVAILABLE)?
            .map(Zeroizing::new)
    } else {
        None
    };
    let route = match &proxy {
        Some(url) => Some(CopyProxy::from_route(url).ok_or(UNAVAILABLE)?),
        None => None,
    };
    let plan = plan(&source, target, operation, route).ok_or(UNAVAILABLE)?;
    let provider = Zeroizing::new(
        secret_store::load_read_only(&root, &source.secret_ref)
            .map_err(|_| STORAGE)?
            .ok_or(UNAVAILABLE)?,
    );
    Ok(CopyInput {
        plan,
        provider,
        proxy,
    })
}

pub(crate) fn load(state: &DesktopState) -> Result<Option<CopyKey>, &'static str> {
    let root = StoragePaths::from_root(&state.root).vault_root();
    secret_store::load_read_only(&root, SLOT)
        .map_err(|_| STORAGE)?
        .map(|v| CopyKey::restore(&Zeroizing::new(v)).ok_or(STORAGE))
        .transpose()
}

pub(crate) fn save(key: &CopyKey) -> Result<(), &'static str> {
    secret_store::save(SLOT, &key.protected_record()).map_err(|_| STORAGE)
}

pub(crate) fn clear() -> Result<(), &'static str> {
    secret_store::delete(SLOT).map_err(|_| STORAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::{
        accounts::proxy::PROXY_POOL_SECRET_REF, models::synthetic_responses_source,
    };

    #[tokio::test]
    async fn read_only_copy_snapshot_keeps_the_saved_source_and_uses_its_explicit_route() {
        let root = crate::files::test_temp_root("hosted-copy");
        let state = DesktopState::open(root.clone()).unwrap();
        let target = Destination::parse(&format!(
            "https://fixture.example/admin/connections#relay-device={}.{}",
            Uuid::new_v4(),
            Uuid::new_v4(),
        ))
        .unwrap();
        let source_id = Uuid::new_v4();
        let mut record = ProviderSourceRecord {
            id: format!("source_{}", source_id.simple()),
            secret_ref: format!("source:{}", source_id.simple()),
            ..synthetic_responses_source()
        };
        let _setup = state.setup_guard().await;
        state
            .store()
            .unwrap()
            .upsert_source(record.clone())
            .unwrap();
        secret_store::save(&record.secret_ref, "fixture-local-api-key").unwrap();

        let proxy_url = "http://fixture-user:fixture-password@127.0.0.1:19000/";
        let mut pool = ProxyPool::default();
        assert_eq!(pool.import(&[proxy_url.to_owned()], 1).unwrap(), (1, 0));
        pool.save().unwrap();
        let incoming = || snapshot(&state, &target, Uuid::from_u128(7), source_id);
        assert_eq!(choices(&state, &target).unwrap().len(), 1);
        assert!(record.effective_protocol_bindings().unwrap().len() > 1);
        let direct = incoming().unwrap();
        assert!(direct.proxy.is_none() && direct.plan.proxy.is_none());
        assert!(direct.provider.as_str() == "fixture-local-api-key");

        record.proxy_id = Some(pool.summary().entries[0].id.clone());
        state
            .store()
            .unwrap()
            .upsert_source(record.clone())
            .unwrap();
        let routed = incoming().unwrap();
        assert!(
            routed
                .proxy
                .as_ref()
                .is_some_and(|v| v.as_str() == proxy_url)
        );
        assert!(
            routed
                .plan
                .proxy
                .is_some_and(|p| p.authenticated && p.endpoint == "http://127.0.0.1:19000")
        );
        assert_eq!(state.store().unwrap().source(&record.id), Some(&record));
        assert!(ProxyPool::load().unwrap() == pool);
        assert!(
            secret_store::load(&record.secret_ref).unwrap().as_deref()
                == Some("fixture-local-api-key")
        );

        record.proxy_id = Some("proxy_missing".to_owned());
        state
            .store()
            .unwrap()
            .upsert_source(record.clone())
            .unwrap();
        assert_eq!(incoming().err(), Some(UNAVAILABLE));
        assert_eq!(state.store().unwrap().source(&record.id), Some(&record));
        record.proxy_id = None;
        record.wire_api = WireApi::ChatCompletions;
        assert!(
            record
                .effective_protocol_bindings()
                .unwrap()
                .iter()
                .any(|r| r.wire_api == WireApi::Responses && !r.adapter.is_passthrough())
        );
        state
            .store()
            .unwrap()
            .upsert_source(record.clone())
            .unwrap();
        assert!(choices(&state, &target).unwrap().is_empty());
        assert_eq!(incoming().err(), Some(UNAVAILABLE));
        secret_store::delete(&record.secret_ref).unwrap();
        secret_store::delete(PROXY_POOL_SECRET_REF).unwrap();
        drop(_setup);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
