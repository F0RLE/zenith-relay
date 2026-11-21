GRANT USAGE ON SCHEMA relay_hosted TO :"inference_role";
GRANT SELECT ON relay_hosted.closed_installation,relay_hosted._sqlx_migrations TO :"inference_role";
REVOKE EXECUTE ON FUNCTION relay_hosted.reserve_text(text,uuid,uuid,bytea,bytea,bytea,bytea,text,integer,integer),
    relay_hosted.claim_text(text,uuid,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer),
    relay_hosted.text_offer_config(uuid,text),
    relay_hosted.claim_published_text(text,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer),
    relay_hosted.reserve_published_text(text,uuid,bytea,bytea,bytea,bytea,text,integer,integer),
    relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint),
    relay_hosted.finish_text(uuid,uuid,uuid,text,bigint,bigint,bigint) FROM :"inference_role";
GRANT EXECUTE ON FUNCTION relay_hosted.qualified_key_workspace(text,uuid,bytea),
    relay_hosted.reserve_published_text(text,uuid,bytea,bytea,bytea,bytea,text,integer,integer,boolean),
    relay_hosted.request_text_candidates(text,uuid,uuid,uuid,bytea),
    relay_hosted.claim_text_candidate(text,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer,uuid,bigint),
    relay_hosted.record_text_cooldown(uuid,uuid,uuid,bigint),
    relay_hosted.key_text_models(text,uuid,bytea),
    relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint,integer),
    relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint,integer,integer,bigint),
    relay_hosted.capture_text_stream_usage(uuid,uuid,uuid,bigint,bigint,bigint),
    relay_hosted.record_request_diagnostic(uuid,uuid,uuid,text,smallint,bigint),
    relay_hosted.reconcile_text_attempt(uuid,uuid,uuid,text),relay_hosted.due_text(),
    relay_hosted.key_text_status(text,uuid,bytea,uuid) TO :"inference_role";
GRANT EXECUTE ON FUNCTION relay_hosted.observe_reconciliation(boolean,boolean) TO :"inference_role";
GRANT EXECUTE ON FUNCTION relay_hosted.capture_target(text,integer,integer,integer,integer),
    relay_hosted.append_capture_milestone(uuid,uuid,uuid,relay_hosted.capture_milestone,bigint) TO :"inference_role";
