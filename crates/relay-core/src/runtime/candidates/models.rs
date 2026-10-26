use super::super::{normalized_set, AccountModelInventory, GatewayRuntime, IMAGE_API_MODEL};
use crate::error_codes;
use crate::CandidateKind;
use std::sync::atomic::Ordering;

impl GatewayRuntime {
    /// Reconcile only the discovered OAuth model inventory. Keep this runtime's
    /// scheduler and executor identity: an inventory read is not fresh auth,
    /// quota or health evidence and must not discard live physical leases.
    /// The scheduler lock also serializes this edit with final dispatch and
    /// model-list projection; old unstarted leases are rechecked there.
    pub fn update_account_models(&self, account_id: &str, models: &[String]) -> bool {
        let Some(account) = self.chatgpt_accounts.get(account_id) else {
            return false;
        };
        let configured_models = normalized_set(models.iter());
        let image_main_model = (account.oauth_client_kind
            == crate::providers::chatgpt::OAuthClientKind::Codex)
            .then(|| IMAGE_API_MODEL.to_owned());
        let mut candidate_models = if account.images_only {
            Default::default()
        } else {
            configured_models.clone()
        };
        let mut published_models = if account.images_only {
            Vec::new()
        } else {
            models.to_vec()
        };
        if image_main_model.is_some() {
            candidate_models.insert(IMAGE_API_MODEL.to_string());
            published_models.push(IMAGE_API_MODEL.to_string());
        }
        let mut scheduler = self.lock_scheduler();
        if scheduler.is_retired() {
            return false;
        }
        let Some(mut candidate) = scheduler.candidate(account_id).cloned() else {
            return false;
        };
        if candidate.kind != CandidateKind::OAuthAccount
            || candidate.account_id.as_deref() != Some(account_id)
        {
            return false;
        }
        candidate.models = candidate_models;
        // Never publish a model that the executor cannot resolve. Hold the
        // scheduler lock until all three views name the same inventory.
        let mut inventory = crate::poison::write(&account.model_inventory);
        let changed = inventory.configured_models != configured_models;
        let image_bridge_changed = changed || inventory.image_main_model != image_main_model;
        *inventory = AccountModelInventory {
            configured_models,
            image_main_model,
        };
        drop(inventory);
        if image_bridge_changed {
            // Inventory changes revoke pending leases, including image sends.
            // The native image model itself never changes with a text model.
            account.image_bridge_revision.fetch_add(1, Ordering::AcqRel);
        }
        crate::poison::mutex(&self.registry).replace(account_id, published_models.iter());
        scheduler.upsert(candidate);
        if changed {
            // Transport cards describe the old inventory.
            // A removed and later reintroduced slug needs fresh evidence.
            crate::poison::mutex(&self.model_metadata.codex_manifests).remove(account_id);
        }
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        self.admission_changed.notify_waiters();
        true
    }

    pub(crate) fn block_candidate_capability(&self, candidate_id: &str, model: &str) -> bool {
        let changed = self.lock_scheduler().block_capability(candidate_id, model);
        if changed {
            self.candidate_availability.notify_waiters();
        }
        changed
    }

    pub(crate) fn clear_candidate_capability_blocks(&self, candidate_id: &str) -> bool {
        let changed = self.lock_scheduler().clear_capability_blocks(candidate_id);
        if changed {
            self.candidate_availability.notify_waiters();
        }
        changed
    }
}
pub(super) fn is_model_capability_failure(category: &str) -> bool {
    matches!(
        category,
        error_codes::UPSTREAM_MODEL_NOT_FOUND
            | error_codes::UPSTREAM_MODEL_UNSUPPORTED
            | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
            | error_codes::IMAGE_GENERATION_NOT_ENABLED
    )
}
