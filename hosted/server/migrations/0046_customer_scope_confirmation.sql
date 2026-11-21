ALTER TABLE relay_hosted.configuration_revisions
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id);

DROP FUNCTION relay_hosted.transition_customer_scope(text,bytea,uuid,bigint,text,text,uuid[],uuid,bytea);

CREATE FUNCTION relay_hosted.transition_customer_scope(origin text, owner_tag bytea, op uuid,
    expected bigint, target text, reason text, candidates uuid[], provider uuid, config_tag bytea,
    proof_credential bytea, proof_revision bigint, proof_counter bigint,
    proof_backup boolean, proof_eligible boolean, proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctl relay_hosted.service_control; sess relay_hosted.admin_sessions; stamp timestamptz; stamp_ms bigint;
    ids uuid[]; prior relay_hosted.configuration_revisions; matched bigint; active_provider uuid; confirmer uuid;
BEGIN
    SELECT s.* INTO ctl FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
        WHERE i.public_origin=origin AND i.setup_state='claimed' AND s.restore_operation IS NULL FOR UPDATE OF i,s;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT ss.* INTO sess FROM relay_hosted.admin_sessions ss
        WHERE ss.verifier=owner_tag AND (relay_hosted.owner_passkey_session(origin,owner_tag)).user_id=ss.user_id
        FOR SHARE;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    IF op IS NULL OR expected IS NULL OR expected NOT BETWEEN 1 AND 9223372036854775806
        OR target IS NULL OR reason IS NULL OR candidates IS NULL
        OR cardinality(candidates)>16 OR array_position(candidates,NULL) IS NOT NULL THEN RETURN 'invalid'; END IF;
    ids := ARRAY(SELECT DISTINCT id FROM unnest(candidates) id ORDER BY id);
    IF cardinality(ids)<>cardinality(candidates) OR NOT (
        (target='qualification' AND reason='verify_customer_access' AND cardinality(ids)>0)
        OR (target='maintenance' AND reason='pause_customer_access' AND cardinality(ids)=0)) THEN RETURN 'invalid'; END IF;

    SELECT * INTO prior FROM relay_hosted.configuration_revisions r WHERE r.id=op;
    IF FOUND THEN
        IF prior.actor_id=sess.user_id AND prior.expected_revision=expected AND prior.reason_code=reason
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
    IF sess.expires_at<=stamp OR sess.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    IF target='qualification' THEN
        SELECT count(*) INTO matched FROM relay_hosted.identity_candidates c
            WHERE c.provider_id=active_provider AND c.id=ANY(ids) AND c.expires_at>stamp;
        IF matched<>cardinality(ids) THEN RETURN 'invalid'; END IF;
    END IF;
    -- Options and receipt lookup use a NULL proof and can never commit an action.
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    stamp_ms := floor(extract(epoch FROM stamp)*1000)::bigint;
    IF proof_deadline_ms IS NULL OR proof_deadline_ms<=stamp_ms OR proof_deadline_ms>stamp_ms+180000
        THEN RETURN 'expired'; END IF;

    UPDATE relay_hosted.admin_credentials SET revision=revision+1, passkey=
        jsonb_set(jsonb_set(jsonb_set(passkey,'{cred,counter}',to_jsonb(proof_counter)),
            '{cred,backup_state}',to_jsonb(proof_backup)),'{cred,backup_eligible}',to_jsonb(proof_eligible))
        WHERE user_id=sess.user_id AND credential_id=proof_credential AND revoked_at IS NULL AND revision=proof_revision
            AND proof_backup IS NOT NULL AND proof_eligible IS NOT NULL AND proof_counter BETWEEN 0 AND 4294967295
            AND (proof_counter>(passkey #>> '{cred,counter}')::bigint
                OR (proof_counter=0 AND (passkey #>> '{cred,counter}')::bigint=0))
        RETURNING passkey_id INTO confirmer;
    IF NOT FOUND THEN RETURN 'passkey_changed'; END IF;

    INSERT INTO relay_hosted.configuration_revisions VALUES(op,expected+1,expected,target,reason,sess.user_id,active_provider,
        ids,stamp,CASE WHEN target='qualification' THEN stamp+interval '24 hours' ELSE stamp END,confirmer);
    IF target='qualification' THEN
        INSERT INTO relay_hosted.qualification_subjects(scope_id, subject_digest)
            SELECT op,c.subject_digest FROM relay_hosted.identity_candidates c WHERE c.provider_id=active_provider AND c.id=ANY(ids);
        DELETE FROM relay_hosted.identity_candidates WHERE provider_id=active_provider;
    END IF;
    UPDATE relay_hosted.service_control SET mode=target,revision=revision+1,admission_epoch=admission_epoch+1,
        qualification_scope_ref=CASE WHEN target='qualification' THEN op END,changed_at=stamp WHERE installation_id=ctl.installation_id;
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.transition_customer_scope(
    text,bytea,uuid,bigint,text,text,uuid[],uuid,bytea,bytea,bigint,bigint,boolean,boolean,bigint) FROM PUBLIC;
