use super::super::*;

impl GatewayRuntime {
    pub(crate) fn executor_route(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        upstream_stream: bool,
    ) -> Option<ExecutorRoute> {
        if !self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| candidate.is_configured(model, allowed_protocols, scope))
        {
            return None;
        }
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            return self.source_executor_route(candidate_id, binding, model, upstream_stream);
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        let source_model = account.canonical_model(model)?;
        Some(Self::account_executor_route(
            account,
            source_model,
            allowed_protocols,
        ))
    }

    pub(crate) fn image_executor_route(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Option<ExecutorRoute> {
        if !self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| candidate.is_configured(model, allowed_protocols, scope))
        {
            return None;
        }
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            if !binding.adapter.is_passthrough() {
                return None;
            }
            return self.source_executor_route(candidate_id, binding, model, false);
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        if account.oauth_client_kind == crate::providers::chatgpt::OAuthClientKind::ExcelBps {
            return None;
        }
        Some(Self::account_executor_route(
            account,
            crate::poison::read(&account.model_inventory)
                .image_main_model
                .clone()?,
            allowed_protocols,
        ))
    }

    fn source_executor_route(
        &self,
        candidate_id: &str,
        binding: &SourceCandidateBinding,
        model: &str,
        upstream_stream: bool,
    ) -> Option<ExecutorRoute> {
        let provider_source = self.sources.get(&binding.source_id)?;
        let source_binding = provider_source.binding_for(binding.binding_key)?;
        let source_model = provider_source.canonical_model_for(binding.binding_key, model)?;
        let route_capability = binding
            .capabilities
            .get(&crate::model_id_key(&source_model))
            .or_else(|| binding.capabilities.get(&crate::model_id_key(model)))
            .cloned();
        Some(ExecutorRoute {
            candidate_id: candidate_id.to_string(),
            source_id: binding.source_id.clone(),
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            client_wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: provider_source.endpoint(
                binding.binding_key,
                &source_model,
                upstream_stream,
            )?,
            upstream_headers: provider_source.protocol_headers_for_binding(source_binding),
            account_transport: AccountTransport::NativeResponses,
            client_transport: crate::UsageTransport::Http,
            source_model,
            route_capability,
            half_open_probe: false,
            routing: None,
            cache_context_observation: None,
        })
    }

    fn account_executor_route(
        account: &ChatGptAccountExecutor,
        source_model: String,
        allowed_protocols: &[WireApi],
    ) -> ExecutorRoute {
        let wire_api = allowed_protocols
            .first()
            .copied()
            .unwrap_or(WireApi::Responses);
        let adapter =
            SourceAdapter::between(wire_api, WireApi::Responses).expect("registered account route");
        let account_transport =
            if account.oauth_client_kind == crate::providers::chatgpt::OAuthClientKind::ExcelBps {
                AccountTransport::ExcelBasisPoints
            } else {
                AccountTransport::NativeResponses
            };
        ExecutorRoute {
            candidate_id: account.id.clone(),
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            account_token_generation: None,
            client_context_id: None,
            client_wire_api: wire_api,
            adapter,
            reasoning_mode: if adapter.is_passthrough() {
                MessagesReasoningMode::Disabled
            } else {
                MessagesReasoningMode::Adaptive
            },
            cache_write_ttl: CacheWriteTtl::Provider,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: match account_transport {
                AccountTransport::NativeResponses => account.responses_url.clone(),
                AccountTransport::ExcelBasisPoints => account.basis_points_url.clone(),
            },
            upstream_headers: match account_transport {
                AccountTransport::NativeResponses => HeaderMap::new(),
                AccountTransport::ExcelBasisPoints => basis_points_headers(
                    &account.chatgpt_account_id,
                    account.chatgpt_user_id.as_deref(),
                    account.basis_points_headers.as_ref(),
                ),
            },
            account_transport,
            client_transport: crate::UsageTransport::Http,
            source_model,
            route_capability: None,
            half_open_probe: false,
            routing: None,
            cache_context_observation: None,
        }
    }
}
