GRANT EXECUTE ON FUNCTION relay_hosted.owner_login_context(text),
    relay_hosted.owner_verifier(text,uuid,bytea,bigint,bigint),
    relay_hosted.issue_admin_session(text,uuid,bytea,bigint,bigint,bigint,bigint,bigint,boolean,boolean,bytea),
    relay_hosted.read_admin_overview(text,bytea),relay_hosted.owner_usage_overview(text,bytea,integer,text),
    relay_hosted.owner_request_page(text,bytea,integer,bigint,bigint,relay_hosted.owner_request_filter,uuid,boolean),
    relay_hosted.owner_request_detail(text,bytea,uuid),
    relay_hosted.prune_request_diagnostics(),
    relay_hosted.end_admin_session(bytea) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_operations_status(text,bytea),
    relay_hosted.owner_operations_events(text,bytea,bigint),
    relay_hosted.observe_projection_cleanup(boolean,boolean,boolean,boolean,boolean,boolean) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_diagnostic_captures(text,bytea,uuid),
    relay_hosted.start_diagnostic_capture(text,bytea,uuid,uuid,relay_hosted.capture_reason,integer,integer,integer,integer,integer),
    relay_hosted.stop_diagnostic_capture(text,bytea,uuid),
    relay_hosted.sweep_diagnostic_capture(),relay_hosted.prune_diagnostic_captures(),
    relay_hosted.prune_capture_events(),relay_hosted.reset_owner_browser_sessions(text) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.customer_login_context(text),
    relay_hosted.customer_binding_matches(uuid,text,text,bytea),
    relay_hosted.finish_customer_login(text,uuid,text,text,bytea,bytea,bigint,bigint,bytea),
    relay_hosted.customer_account(text,uuid,bytea,bytea),relay_hosted.end_customer_session(bytea),
    relay_hosted.owner_service_view(text,bytea),
    relay_hosted.transition_customer_scope(text,bytea,uuid,bigint,text,text,uuid[],uuid,bytea) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.read_customer_keys(text,uuid,bytea,bytea,uuid,uuid),
    relay_hosted.issue_customer_key(text,uuid,bytea,bytea,uuid,text,uuid,bigint,uuid,bytea),
    relay_hosted.revoke_customer_key(text,uuid,bytea,bytea,uuid) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.customer_history(text,uuid,bytea,bytea,text,integer,uuid,uuid) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.customer_checkout_methods(text,uuid,bytea,bytea),
    relay_hosted.quote_checkout(text,uuid,bytea,bytea,uuid,bigint),
    relay_hosted.confirm_checkout(text,uuid,bytea,bytea,uuid,uuid),
    relay_hosted.customer_checkout_history(text,uuid,bytea,bytea,uuid,uuid,uuid),
    relay_hosted.customer_checkout_resume(text,uuid,bytea,bytea,uuid,uuid,boolean),
    relay_hosted.prune_checkout_quotes(),relay_hosted.prune_checkout_resume_checks() TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_checkout_builder(text,bytea),
    relay_hosted.change_checkout_builder(text,bytea,uuid,bigint,text,jsonb,uuid) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.customer_wallet(text,uuid,bytea,bytea),
    relay_hosted.request_payment_refund(text,uuid,bytea,bytea,uuid,uuid,bigint),
    relay_hosted.owner_refund_cases(text,bytea,uuid,uuid),
    relay_hosted.review_payment_refund(text,bytea,uuid,uuid,text,text) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.customer_model_status(text,uuid,bytea,bytea,integer) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_service_notices(text,bytea),
    relay_hosted.change_service_notice(text,bytea,uuid,uuid,integer,text,text,text),
    relay_hosted.customer_service_notices(text,uuid,bytea,bytea),
    relay_hosted.read_service_notices(text,uuid,bytea,bytea,uuid[]),
    relay_hosted.prune_service_notices() TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.start_usage_report(text,uuid,bytea,bytea,integer),
    relay_hosted.read_usage_report(text,uuid,bytea,bytea,uuid,relay_hosted.usage_filter,uuid,uuid,text),
    relay_hosted.prune_usage_reports() TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_text_rules(text,bytea),relay_hosted.change_text_rule(text,bytea,uuid,uuid,bigint,text,jsonb),
    relay_hosted.owner_text_price_inputs(text,bytea,uuid,boolean),
    relay_hosted.customer_text_models(text,uuid,bytea,bytea) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_connections(text,bytea,uuid) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_purchase_limit(text,bytea,uuid),
    relay_hosted.change_purchase_limit(text,bytea,uuid,uuid,bigint,bigint) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.owner_devices(text,bytea),relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea),
    relay_hosted.device_connections(text,uuid,uuid,bytea,uuid) TO :"runtime_role";
GRANT EXECUTE ON FUNCTION relay_hosted.review_source_copy(text,bytea,uuid,uuid,uuid,uuid,jsonb,bytea,bytea,boolean) TO :"runtime_role";
