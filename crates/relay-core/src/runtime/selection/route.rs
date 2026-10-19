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
        self.executor_routes(
            candidate_id,
            model,
            scope,
            allowed_protocols,
            upstream_stream,
        )
        .into_iter()
        .next()
    }

    pub(crate) fn executor_routes(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        upstream_stream: bool,
    ) -> Vec<ExecutorRoute> {
        if !self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| candidate.is_configured(model, allowed_protocols, scope))
        {
            return Vec::new();
        }
        // The first protocol is the client's native wire API. It is first
        // because candidate_protocols returns it before cross-protocol
        // candidates, and single-protocol callers pass &[client].
        let client_wire_api = allowed_protocols
            .first()
            .copied()
            .unwrap_or(WireApi::Responses);
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let catalog = self
                .model_metadata_catalog
                .as_ref()
                .map(|catalog| catalog.snapshot());
            let preferences = catalog
                .as_ref()
                .map(|catalog| catalog.protocol_preferences_for(model))
                .unwrap_or_default();
            let mut routes = binding
                .routes
                .iter()
                .filter(|route| {
                    route.wire_api == client_wire_api
                        && route.models.contains(&crate::model_id_key(model))
                })
                .filter_map(|route| {
                    self.source_executor_route(candidate_id, binding, route, model, upstream_stream)
                })
                .filter(|route| {
                    catalog
                        .as_ref()
                        .and_then(|catalog| catalog.native_protocol_for(&route.source_model))
                        .is_none_or(|native| {
                            route
                                .adapter
                                .upstream_protocol(route.client_wire_api)
                                .wire_api()
                                == native
                        })
                })
                .collect::<Vec<_>>();
            routes.sort_by_key(|route| {
                let upstream = route
                    .adapter
                    .upstream_protocol(route.client_wire_api)
                    .wire_api();
                (
                    allowed_protocols
                        .iter()
                        .position(|protocol| *protocol == route.client_wire_api),
                    preferences
                        .iter()
                        .position(|protocol| *protocol == upstream)
                        .unwrap_or(usize::MAX),
                    !route.adapter.is_passthrough(),
                    upstream,
                )
            });
            return routes;
        }
        let Some(account) = self.chatgpt_accounts.get(candidate_id) else {
            return Vec::new();
        };
        let Some(source_model) = account.canonical_model(model) else {
            return Vec::new();
        };
        vec![Self::account_executor_route(
            account,
            source_model,
            allowed_protocols,
        )]
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
            return binding
                .routes
                .iter()
                .filter(|route| {
                    route.adapter.is_passthrough() && allowed_protocols.contains(&route.wire_api)
                })
                .find_map(|route| {
                    self.source_executor_route(candidate_id, binding, route, model, false)
                });
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
        source_route: &SourceCandidateRoute,
        model: &str,
        upstream_stream: bool,
    ) -> Option<ExecutorRoute> {
        let provider_source = self.sources.get(&binding.source_id)?;
        let source_binding = provider_source.binding_for(source_route.binding_key)?;
        let source_model = provider_source.canonical_model_for(source_route.binding_key, model)?;
        // Resolution already projects upstreams onto each supported client API.
        // Preserve this exact binding for headers, streaming and continuations;
        // reinterpreting another projection would duplicate or change its identity.
        let route_capability = source_route
            .capabilities
            .get(&crate::model_id_key(&source_model))
            .or_else(|| source_route.capabilities.get(&crate::model_id_key(model)))
            .cloned();
        Some(ExecutorRoute {
            candidate_id: candidate_id.to_string(),
            source_id: binding.source_id.clone(),
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            client_wire_api: source_route.wire_api,
            adapter: source_route.adapter,
            reasoning_mode: source_route.reasoning_mode,
            reasoning_effort_override: None,
            cache_write_ttl: source_route.cache_write_ttl,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: provider_source.endpoint(
                source_route.binding_key,
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

    /// Resolve the transport for the already selected binding. Compaction can
    /// change streaming, but cannot select another protocol mid-request.
    pub(crate) fn set_route_streaming(&self, route: &mut ExecutorRoute, stream: bool) {
        if let Some(source) = self.sources.get(&route.source_id) {
            if let Some(url) = source.endpoint(route.binding_key(), &route.source_model, stream) {
                route.upstream_url = url;
            }
        }
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
            reasoning_effort_override: None,
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
