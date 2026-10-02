//! One projection for stored records that use the same field names.
//! A nested account record cannot use these macros: its flags live under
//! `account`, not on the record itself.

#[macro_export]
macro_rules! impl_source_protocol_resolution {
    ($ty:ty) => {
        impl $crate::SourceProtocolResolution for $ty {
            fn protocol_base_url(&self) -> &str {
                &self.base_url
            }

            fn protocol_models(&self) -> &[String] {
                &self.models
            }

            fn stored_protocol_bindings(&self) -> &[$crate::SourceProtocolBinding] {
                &self.protocol_bindings
            }

            fn protocol_fallback(&self) -> $crate::WireApi {
                self.wire_api
            }

            fn source_protocol_config(&self) -> &$crate::SourceProtocolConfig {
                &self.protocol_config
            }
        }
    };
}

#[macro_export]
macro_rules! impl_pool_participant {
    ($ty:ty) => {
        impl $crate::PoolParticipant for $ty {
            fn pool_access(&self) -> $crate::PoolAccess<'_> {
                $crate::PoolAccess {
                    enabled: self.enabled,
                    in_pool: self.in_pool,
                    draining: self.draining,
                    allowed_models: &self.allowed_models,
                    excluded_models: &self.excluded_models,
                }
            }
        }
    };
}

#[macro_export]
macro_rules! impl_account_operational_source {
    ($ty:ty) => {
        impl $crate::protocol::AccountOperationalSource for $ty {
            fn operational_enabled(&self) -> bool {
                self.enabled
            }
            fn operational_in_pool(&self) -> bool {
                self.in_pool
            }
            fn operational_draining(&self) -> bool {
                self.draining
            }
            fn operational_auth_state(&self) -> $crate::accounts::AccountAuthState {
                self.auth_state
            }
            fn operational_health(&self) -> $crate::accounts::AccountHealthState {
                self.health
            }
            fn operational_subscription(&self) -> &$crate::quota::Subscription {
                &self.subscription
            }
            fn operational_quota(&self) -> &$crate::quota::QuotaSnapshot {
                &self.quota
            }
            fn operational_last_error_code(&self) -> Option<&str> {
                self.last_error_code.as_deref()
            }
        }
    };
}

/// A successful discovery list replaces the configured model list.
#[macro_export]
macro_rules! impl_effective_models {
    ($ty:ty) => {
        impl $ty {
            pub fn effective_models(&self) -> &[String] {
                self.discovered_models.as_deref().unwrap_or(&self.models)
            }
        }
    };
}

#[macro_export]
macro_rules! impl_stored_source_record {
    ($ty:ty) => {
        $crate::impl_source_protocol_resolution!($ty);
        $crate::impl_pool_participant!($ty);

        impl $crate::SourceTransportRecord for $ty {
            fn transport_identity(&self) -> $crate::SourceTransportIdentity<'_> {
                $crate::SourceTransportIdentity {
                    id: &self.id,
                    base_url: &self.base_url,
                    secret_ref: &self.secret_ref,
                    wire_api: self.wire_api,
                    protocol_bindings: &self.protocol_bindings,
                    protocol_config: &self.protocol_config,
                    models: &self.models,
                }
            }
        }

        impl $crate::SourceCatalogRecord for $ty {
            fn catalog_evidence(&self) -> $crate::SourceCatalogEvidence<'_> {
                $crate::SourceCatalogEvidence {
                    base_url: &self.base_url,
                    models: &self.models,
                    protocol_bindings: &self.protocol_bindings,
                    protocol_config: &self.protocol_config,
                    detected_model_prices: &self.detected_model_prices,
                }
            }
        }

        impl $crate::RuntimeSourcePolicyRecord for $ty {
            fn runtime_source_policy_update(&self) -> $crate::RuntimeSourcePolicyUpdate {
                $crate::RuntimeSourcePolicyUpdate {
                    source_id: self.id.clone(),
                    policy: $crate::RuntimeCandidatePolicy {
                        enabled: self.enabled,
                        draining: self.draining,
                        priority: self.priority,
                        weight: self.weight,
                        allowed_models: self.allowed_models.clone(),
                        excluded_models: self.excluded_models.clone(),
                    },
                    recovery_delay_seconds: self.recovery_delay_seconds,
                }
            }
        }

        impl $crate::protocol::SourceSummaryRecord for $ty {
            fn summary_id(&self) -> &str {
                &self.id
            }
            fn summary_name(&self) -> &str {
                &self.name
            }
            fn summary_enabled(&self) -> bool {
                self.enabled
            }
            fn summary_in_pool(&self) -> bool {
                self.in_pool
            }
            fn summary_draining(&self) -> bool {
                self.draining
            }
            fn summary_pricing_provider(&self) -> Option<&str> {
                self.pricing_provider.as_deref()
            }
            fn summary_official_provider_family(&self) -> Option<&str> {
                self.official_provider_family.as_deref()
            }
            fn summary_priority(&self) -> i32 {
                self.priority
            }
            fn summary_weight(&self) -> u32 {
                self.weight
            }
            fn summary_recovery_delay_seconds(&self) -> u64 {
                self.recovery_delay_seconds
            }
            fn summary_allowed_models(&self) -> &[String] {
                &self.allowed_models
            }
            fn summary_excluded_models(&self) -> &[String] {
                &self.excluded_models
            }
            fn summary_model_price_overrides(
                &self,
            ) -> &::std::collections::BTreeMap<String, $crate::ApiModelPriceOverride> {
                &self.model_price_overrides
            }
            fn summary_detected_model_prices(
                &self,
            ) -> &::std::collections::BTreeMap<String, $crate::ApiModelPriceOverride> {
                &self.detected_model_prices
            }
        }
    };
}
