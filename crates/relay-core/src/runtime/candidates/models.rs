use super::super::{AccountModelInventory, GatewayRuntime, normalized_set};
use crate::CandidateKind;
use crate::error_codes;
use std::sync::atomic::Ordering;

impl GatewayRuntime {
    /// Reconcile only the discovered OAuth model inventory. Keep this runtime's
    /// scheduler and executor identity: an inventory read is not fresh auth,
    /// quota or health evidence and must not discard live physical leases.
    /// The scheduler lock also serializes this edit with final dispatch and
    /// model-list projection; old unstarted leases are rechecked there.
    pub fn update_account_models(&self, account_id: &str, models: &[String]) -> bool {
        self.reconcile_account_models(account_id, Some(models))
    }

    pub(in crate::runtime) fn refresh_account_image_models(&self, account_id: &str) -> bool {
        self.reconcile_account_models(account_id, None)
    }

    fn reconcile_account_models(&self, account_id: &str, models: Option<&[String]>) -> bool {
        let Some(account) = self.account_state.executors.get(account_id) else {
            return false;
        };
        let plan = crate::poison::read(&account.subscription_plan_type);
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
        let mut inventory = crate::poison::write(&account.model_inventory);
        let configured_models = models
            .map(|models| normalized_set(models.iter()))
            .unwrap_or_else(|| inventory.configured_models.clone());
        let images_available = super::super::images::account_images_available(
            account.oauth_client_kind,
            plan.as_deref(),
        );
        let models = configured_models
            .iter()
            .filter(|model| {
                super::super::images::account_model_enabled(
                    account.oauth_client_kind,
                    model,
                    plan.as_deref(),
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut candidate_models = if account.images_only {
            Default::default()
        } else {
            normalized_set(models.iter())
        };
        let mut published_models = if account.images_only {
            Vec::new()
        } else {
            models.clone()
        };
        candidate_models.extend(
            super::super::images::account_image_models(account.oauth_client_kind)
                .filter(|_| images_available)
                .map(str::to_owned),
        );
        published_models.extend(
            super::super::images::account_image_models(account.oauth_client_kind)
                .filter(|_| images_available)
                .map(str::to_owned),
        );
        candidate.models = candidate_models;
        candidate.native_images = images_available;
        // Never publish a model that the executor cannot resolve. Hold the
        // scheduler lock until all three views name the same inventory.
        let changed = inventory.configured_models != configured_models;
        *inventory = AccountModelInventory { configured_models };
        drop(inventory);
        if changed {
            // Inventory changes revoke pending leases, including image sends.
            // The native image model itself never changes with a text model.
            account.image_bridge_revision.fetch_add(1, Ordering::AcqRel);
        }
        crate::poison::mutex(&self.model_state.registry)
            .replace(account_id, published_models.iter());
        scheduler.upsert(candidate);
        if changed {
            // Transport cards describe the old inventory.
            // A removed and later reintroduced slug needs fresh evidence.
            crate::poison::mutex(&self.provider_metadata.codex_manifests).remove(account_id);
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
