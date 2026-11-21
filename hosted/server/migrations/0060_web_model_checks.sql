-- The web intake accepts up to 80 UTF-16 units, including non-ASCII labels.
ALTER TABLE relay_hosted.text_inventory DROP CONSTRAINT text_inventory_source_label_check,
    ADD CHECK (octet_length(source_label) BETWEEN 1 AND 240 AND source_label !~ '[[:cntrl:]]');

CREATE TABLE relay_hosted.web_model_checks (
    operation_id uuid PRIMARY KEY,
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    authorization_epoch bigint NOT NULL,
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    source_id uuid NOT NULL,
    source_revision bigint NOT NULL,
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    confirmed_by_passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    profile jsonb NOT NULL,
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    started_at timestamptz,
    finished_at timestamptz
);
CREATE INDEX web_model_check_source_history ON relay_hosted.web_model_checks
    (installation_id,source_id,approved_at DESC,operation_id DESC);
CREATE INDEX web_model_check_queue ON relay_hosted.web_model_checks
    (installation_id,scope_id,approved_at,operation_id) WHERE started_at IS NULL;

CREATE FUNCTION relay_hosted.approve_web_model_check(origin text,tag bytea,source uuid,op uuid,expected bigint,
    inputs jsonb,snapshot text,proof_credential bytea,proof_revision bigint,proof_counter bigint,
    proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE sess relay_hosted.admin_sessions; inst relay_hosted.installations; prior relay_hosted.web_model_checks;
    stamp timestamptz; until_at timestamptz; scope uuid; outcome text; signer uuid; profile jsonb; binding text;
BEGIN
    IF origin NOT LIKE 'https://%' OR source IS NULL OR op IS NULL OR source='00000000-0000-0000-0000-000000000000'
        OR op='00000000-0000-0000-0000-000000000000' OR expected IS NULL OR expected<0
        OR jsonb_typeof(inputs) IS DISTINCT FROM 'object' OR octet_length(inputs::text)>512
        OR NOT EXISTS(SELECT FROM jsonb_to_record(inputs)
            AS i(model text,max_input_bytes integer,max_output_tokens integer,tokens_per_byte integer,overhead_tokens integer)
            WHERE i.model ~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$' AND i.max_input_bytes BETWEEN 14 AND 60000
            AND i.max_output_tokens BETWEEN 1 AND 1000000 AND i.tokens_per_byte BETWEEN 1 AND 16
            AND i.overhead_tokens BETWEEN 0 AND 65536)
        THEN RETURN '{"state":"invalid"}'; END IF;
    sess:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF sess.verifier IS NULL THEN RETURN '{"state":"denied"}'; END IF;
    SELECT * INTO STRICT inst FROM relay_hosted.installations WHERE public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('web-model-checks:'||inst.id::text,0));
    stamp:=clock_timestamp();
    IF sess.expires_at<=stamp OR sess.last_seen_at<=stamp-interval '20 minutes' THEN RETURN '{"state":"denied"}'; END IF;
    SELECT * INTO prior FROM relay_hosted.web_model_checks w WHERE w.operation_id=op;
    IF FOUND THEN
        RETURN CASE WHEN (prior.installation_id,prior.actor,prior.source_id,prior.source_revision)
            =(inst.id,sess.user_id,source,expected) AND prior.profile @> inputs
            THEN '{"state":"approved"}'::jsonb ELSE '{"state":"conflict"}'::jsonb END;
    END IF;
    SELECT q.id,q.expires_at INTO scope,until_at FROM relay_hosted.service_control s
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE s.installation_id=inst.id AND s.mode='qualification';
    IF scope IS NULL OR until_at<=stamp THEN RETURN '{"state":"conflict"}'; END IF;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    -- Resolve only a saved route. The browser cannot select an endpoint, proxy,
    -- scope, or credential revision in a check command.
    SELECT inputs||jsonb_build_object('id',op,'scope',scope,'source',source,'source_label',w.label,'endpoint',w.endpoint,
        'provider_revision',k.revision,'proxy_revision',coalesce(p.revision,0),'uses_proxy',p.generation IS NOT NULL,
        'close_after_s',65,'valid_for_s',3600) INTO profile
    FROM relay_hosted.web_source_intakes w
    JOIN relay_hosted.connection_secret_slots k
        ON (k.installation_id,k.connection_id,k.purpose)=(w.installation_id,w.source_id,'provider') AND k.generation IS NOT NULL
    LEFT JOIN relay_hosted.connection_secret_slots p
        ON (p.installation_id,p.connection_id,p.purpose)=(w.installation_id,w.source_id,'proxy')
    LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(w.installation_id,w.source_id)
    WHERE (w.installation_id,w.source_id)=(inst.id,source) AND w.committed_at IS NOT NULL
        AND w.proxied=(p.generation IS NOT NULL) AND coalesce(v.revision,0)=expected AND NOT coalesce(v.paused,false);
    IF profile IS NULL THEN RETURN '{"state":"conflict"}'; END IF;
    IF EXISTS(SELECT FROM relay_hosted.current_text_inventory i
        WHERE (i.scope_id,i.source_id,i.model)=(scope,source,inputs->>'model'))
        THEN RETURN '{"state":"already_verified"}'; END IF;
    IF EXISTS(SELECT FROM relay_hosted.web_model_checks w WHERE (w.installation_id,w.scope_id,w.source_id)=(inst.id,scope,source)
        AND w.finished_at IS NULL AND coalesce(w.started_at+interval '2 minutes',w.expires_at)>clock_timestamp())
        THEN RETURN '{"state":"busy"}'; END IF;
    IF (SELECT count(*) FROM relay_hosted.web_model_checks w WHERE w.scope_id=scope)>=128
        OR (SELECT count(*) FROM relay_hosted.web_model_checks w WHERE w.scope_id=scope AND w.started_at IS NULL
            AND w.expires_at>clock_timestamp())>=16 THEN RETURN '{"state":"limit"}'; END IF;
    binding:=encode(sha256(convert_to(jsonb_build_array(inst.authorization_epoch,expected,profile)::text,'UTF8')),'hex');
    IF proof_credential IS NULL THEN RETURN jsonb_build_object('state','fresh_required','snapshot',binding); END IF;
    IF snapshot IS DISTINCT FROM binding THEN RETURN '{"state":"conflict"}'; END IF;
    SELECT c.outcome,c.confirmer,c.verified_at INTO outcome,signer,stamp FROM relay_hosted.confirm_owner_payment(
        sess,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms) c;
    IF outcome<>'verified' THEN RETURN jsonb_build_object('state',outcome); END IF;
    IF until_at<=stamp THEN RETURN '{"state":"conflict"}'; END IF;
    INSERT INTO relay_hosted.web_model_checks(operation_id,installation_id,authorization_epoch,scope_id,source_id,
        source_revision,actor,confirmed_by_passkey_id,profile,approved_at,expires_at)
        VALUES(op,inst.id,inst.authorization_epoch,scope,source,expected,sess.user_id,signer,profile,
            stamp,least(until_at,stamp+interval '2 minutes'));
    RETURN '{"state":"approved"}';
END $$;

CREATE FUNCTION relay_hosted.owner_web_model_check(origin text,tag bytea,source uuid,op uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF (relay_hosted.owner_passkey_session(origin,tag)).verifier IS NULL THEN RETURN NULL; END IF;
    RETURN jsonb_build_object('check',(
        SELECT jsonb_build_object('operation_id',w.operation_id,'model',w.profile->>'model',
            'state', CASE WHEN EXISTS(SELECT FROM relay_hosted.text_inventory i WHERE i.id=w.operation_id) THEN 'verified'
                WHEN w.finished_at IS NOT NULL THEN 'not_verified'
                WHEN w.started_at IS NOT NULL THEN
                    CASE WHEN w.started_at+interval '2 minutes'>clock_timestamp() THEN 'running' ELSE 'unconfirmed' END
                WHEN w.expires_at<=clock_timestamp() OR relay_hosted.text_check_scope(origin) IS DISTINCT FROM w.scope_id
                    OR w.authorization_epoch<>h.authorization_epoch THEN 'expired'
                ELSE 'queued' END)
        FROM relay_hosted.web_model_checks w JOIN relay_hosted.installations h ON h.id=w.installation_id
        WHERE h.public_origin=origin AND w.actor=h.owner_user_id AND w.source_id=source AND (op IS NULL OR w.operation_id=op)
        ORDER BY w.approved_at DESC,w.operation_id DESC LIMIT 1));
END $$;

CREATE FUNCTION relay_hosted.take_web_model_check(origin text)
RETURNS jsonb LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH ready AS (
        SELECT w.operation_id FROM relay_hosted.web_model_checks w
        JOIN relay_hosted.installations h ON h.id=w.installation_id AND h.authorization_epoch=w.authorization_epoch
        JOIN relay_hosted.service_control s ON (s.installation_id,s.qualification_scope_ref)=(h.id,w.scope_id)
        WHERE h.public_origin=origin AND s.mode='qualification' AND s.restore_operation IS NULL
            AND w.started_at IS NULL AND w.expires_at>clock_timestamp()
        ORDER BY w.approved_at,w.operation_id LIMIT 1 FOR UPDATE OF w SKIP LOCKED
    )
    UPDATE relay_hosted.web_model_checks w SET started_at=clock_timestamp() FROM ready
        WHERE w.operation_id=ready.operation_id RETURNING w.profile
$$;

CREATE FUNCTION relay_hosted.step_web_model_check(origin text,bound_profile jsonb,step text)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE w relay_hosted.web_model_checks;
BEGIN
    -- Fence lifecycle/restore first, in the same order as text_check_step.
    SELECT q.* INTO w FROM relay_hosted.web_model_checks q
        JOIN relay_hosted.installations h ON h.id=q.installation_id AND h.authorization_epoch=q.authorization_epoch
        JOIN relay_hosted.service_control s ON (s.installation_id,s.qualification_scope_ref)=(h.id,q.scope_id)
        WHERE h.public_origin=origin AND s.mode='qualification' AND s.restore_operation IS NULL
            AND q.operation_id=(bound_profile->>'id')::uuid AND q.profile=bound_profile-'direct_ipv4'
            AND q.started_at IS NOT NULL AND q.finished_at IS NULL AND h.owner_user_id=q.actor
        FOR UPDATE OF s;
    IF NOT FOUND OR w.started_at+interval '2 minutes'<=clock_timestamp()
        OR (step<>'release' AND (
            EXISTS(SELECT FROM relay_hosted.current_text_inventory i
                WHERE (i.scope_id,i.source_id,i.model)=(w.scope_id,w.source_id,bound_profile->>'model'))
            OR w.source_revision<>coalesce((SELECT v.revision FROM relay_hosted.source_serving v
                WHERE (v.installation_id,v.source_id)=(w.installation_id,w.source_id)),0)
            OR NOT EXISTS(SELECT FROM relay_hosted.admin_credentials c WHERE c.passkey_id=w.confirmed_by_passkey_id
                AND c.user_id=w.actor AND c.revoked_at IS NULL))) THEN RETURN 'denied'; END IF;
    RETURN relay_hosted.text_check_step(origin,w.scope_id,w.operation_id,w.source_id,(bound_profile->>'provider_revision')::bigint,
        (bound_profile->>'proxy_revision')::bigint,(bound_profile->>'uses_proxy')::boolean,bound_profile,step);
END $$;

CREATE FUNCTION relay_hosted.finish_web_model_check(origin text,op uuid)
RETURNS void LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    UPDATE relay_hosted.web_model_checks w SET finished_at=clock_timestamp()
        FROM relay_hosted.installations h WHERE h.id=w.installation_id AND h.public_origin=origin
        AND w.operation_id=op AND w.started_at IS NOT NULL AND w.finished_at IS NULL
$$;

REVOKE ALL ON FUNCTION relay_hosted.approve_web_model_check(text,bytea,uuid,uuid,bigint,jsonb,text,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.owner_web_model_check(text,bytea,uuid,uuid),relay_hosted.take_web_model_check(text),
    relay_hosted.step_web_model_check(text,jsonb,text),relay_hosted.finish_web_model_check(text,uuid) FROM PUBLIC;
