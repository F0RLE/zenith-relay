CREATE FUNCTION relay_hosted.owner_service_view(origin text, owner_tag bytea)
RETURNS TABLE(mode text,revision bigint)
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT s.mode,s.revision FROM relay_hosted.service_control s
    JOIN relay_hosted.installations i ON i.id=s.installation_id
    WHERE i.public_origin=origin AND relay_hosted.read_admin_overview(origin,owner_tag) IS NOT NULL
$$;

-- The recent UV passkey session, reviewed candidates, admission epoch and audit
-- are checked under the same locks as the transition. Nothing here can open live.
CREATE FUNCTION relay_hosted.transition_customer_scope(origin text, owner_tag bytea, op uuid,
    expected bigint, target text, reason text, candidates uuid[], provider uuid, config_tag bytea)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctl relay_hosted.service_control; actor uuid; proof_at timestamptz; stamp timestamptz;
    ids uuid[]; prior relay_hosted.configuration_revisions; matched bigint; active_provider uuid;
BEGIN
    SELECT s.* INTO ctl FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
        WHERE i.public_origin=origin AND i.setup_state='claimed' FOR UPDATE OF s;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT a.user_id,ss.created_at INTO actor,proof_at FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions ss ON ss.user_id=a.user_id AND ss.credential_id=a.credential_id
        WHERE ss.verifier=owner_tag AND a.public_origin=origin AND ss.installation_epoch=a.installation_epoch
            AND ss.user_epoch=a.user_epoch AND ss.expires_at>clock_timestamp()
            AND ss.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF ss,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    IF op IS NULL OR expected IS NULL OR target IS NULL OR reason IS NULL OR candidates IS NULL
        OR cardinality(candidates)>16 OR array_position(candidates,NULL) IS NOT NULL THEN RETURN 'invalid'; END IF;
    ids := ARRAY(SELECT DISTINCT id FROM unnest(candidates) id ORDER BY id);
    IF cardinality(ids)<>cardinality(candidates) OR NOT (
        (target='qualification' AND reason='verify_customer_access' AND cardinality(ids)>0)
        OR (target='maintenance' AND reason='pause_customer_access' AND cardinality(ids)=0)) THEN RETURN 'invalid'; END IF;
    SELECT * INTO prior FROM relay_hosted.configuration_revisions r WHERE r.id=op;
    IF FOUND THEN
        IF prior.actor_id=actor AND prior.expected_revision=expected AND prior.reason_code=reason
            AND prior.mode=target AND prior.candidate_ids=ids
            AND (target='maintenance' OR prior.provider_id=provider) THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF expected<>ctl.revision THEN RETURN 'conflict'; END IF;
    IF target='qualification' AND ctl.mode IN ('setup','maintenance') THEN
        active_provider := provider;
        PERFORM FROM relay_hosted.identity_providers p WHERE p.id=provider AND p.binding=config_tag FOR UPDATE;
        IF NOT FOUND THEN RETURN 'invalid'; END IF;
    ELSIF target='maintenance' AND ctl.mode='qualification' THEN
        SELECT r.provider_id INTO STRICT active_provider FROM relay_hosted.configuration_revisions r WHERE r.id=ctl.qualification_scope_ref;
    ELSE RETURN 'conflict'; END IF;
    stamp := clock_timestamp();
    IF proof_at<=stamp-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    IF target='qualification' THEN
        SELECT count(*) INTO matched FROM relay_hosted.identity_candidates c
            WHERE c.provider_id=active_provider AND c.id=ANY(ids) AND c.expires_at>stamp;
        IF matched<>cardinality(ids) THEN RETURN 'invalid'; END IF;
    END IF;
    INSERT INTO relay_hosted.configuration_revisions VALUES(op,expected+1,expected,target,reason,actor,active_provider,
        ids,stamp,CASE WHEN target='qualification' THEN stamp+interval '24 hours' ELSE stamp END);
    IF target='qualification' THEN
        INSERT INTO relay_hosted.qualification_subjects(scope_id, subject_digest)
            SELECT op,c.subject_digest FROM relay_hosted.identity_candidates c WHERE c.provider_id=active_provider AND c.id=ANY(ids);
        DELETE FROM relay_hosted.identity_candidates WHERE provider_id=active_provider;
    END IF;
    UPDATE relay_hosted.service_control SET mode=target,revision=revision+1,admission_epoch=admission_epoch+1,
        qualification_scope_ref=CASE WHEN target='qualification' THEN op END,changed_at=stamp WHERE installation_id=ctl.installation_id;
    RETURN 'applied';
END $$;
REVOKE ALL ON FUNCTION relay_hosted.owner_service_view(text,bytea),
    relay_hosted.transition_customer_scope(text,bytea,uuid,bigint,text,text,uuid[],uuid,bytea) FROM PUBLIC;
