ALTER TABLE relay_hosted.source_serving_changes DROP CONSTRAINT source_serving_changes_action_check,
    ADD CHECK (action IN ('pause','resume','park','trash','restore','remove'));

CREATE OR REPLACE FUNCTION relay_hosted.change_source_serving(origin text,tag bytea,source uuid,op uuid,expected bigint,act text,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; scope uuid; prior relay_hosted.source_serving_changes; v relay_hosted.source_serving;
    epoch bigint; model_id text; proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF source IS NULL OR source='00000000-0000-0000-0000-000000000000'
        OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 0 AND 9223372036854775806
        OR act IS NULL OR act NOT IN ('pause','resume','park','trash','restore','remove') THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT h.id,s.qualification_scope_ref INTO STRICT iid,scope
    FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id WHERE h.public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('source-serving-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes' THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.source_serving_changes c WHERE c.installation_id=iid AND c.operation_id=op;
    IF FOUND THEN RETURN CASE WHEN (prior.source_id,prior.revision-1,prior.actor,
        coalesce(prior.action,CASE WHEN prior.paused THEN 'pause' ELSE 'resume' END))
        =(source,expected,ss.user_id,act) THEN 'applied' ELSE 'conflict' END; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true THEN RETURN 'passkey_changed'; END IF;

    -- Match the vault-before-source order of dispatch/qualification. Only the
    -- confirmed removal takes the exclusive vault lock; the options read does not.
    IF act='remove' AND proof_credential IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(98653249,1);
    END IF;
    FOR model_id IN SELECT DISTINCT i.model FROM relay_hosted.text_inventory i
        WHERE (i.scope_id,i.source_id)=(scope,source) ORDER BY i.model
    LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||scope::text||':'||model_id,0));
    END LOOP;
    -- Dispatch may hold rule row locks while waiting here; never write them.
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    PERFORM FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=iid AND s.connection_id=source
        AND s.purpose='provider' AND s.generation IS NOT NULL FOR SHARE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO v FROM relay_hosted.source_serving s WHERE (s.installation_id,s.source_id)=(iid,source);
    epoch:=coalesce(v.inventory_epoch,0);
    IF coalesce(v.revision,0)<>expected OR (v.action IS NOT DISTINCT FROM 'trash')<>(act IN ('restore','remove'))
        OR (act='pause' AND v.paused) OR (act='resume' AND NOT coalesce(v.paused,false))
        OR (act='park' AND NOT coalesce(relay_hosted.source_in_pool(scope,source,epoch),false)) THEN RETURN 'conflict'; END IF;
    IF act IN ('resume','remove') AND (relay_hosted.text_source_in_flight(source) OR relay_hosted.text_probe_in_flight(source))
        THEN RETURN 'draining'; END IF;
    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;

    IF act='remove' THEN
        UPDATE relay_hosted.connection_secret_slots SET generation=NULL,manifest_digest=NULL,revision=revision+1
            WHERE (installation_id,connection_id)=(iid,source) AND generation IS NOT NULL;
        -- Reserve even an unused proxy slot so a late writer cannot attach a
        -- secret to a removed source. Ciphertext cleanup remains vault-only.
        INSERT INTO relay_hosted.connection_secret_slots(installation_id,connection_id,purpose,revision)
            VALUES(iid,source,'proxy',1) ON CONFLICT DO NOTHING;
    END IF;
    INSERT INTO relay_hosted.source_serving_changes(installation_id,source_id,revision,operation_id,actor,paused,changed_at,
        confirmed_by_passkey_id,action,inventory_epoch)
        VALUES(iid,source,expected+1,op,ss.user_id,act<>'resume',stamp,confirmed,act,
            CASE WHEN act IN ('park','trash','restore','remove') THEN expected+1 ELSE epoch END);
    RETURN 'applied';
END $$;
