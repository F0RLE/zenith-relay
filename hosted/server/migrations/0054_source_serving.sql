CREATE TABLE relay_hosted.source_serving_changes (
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    operation_id uuid NOT NULL,
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    paused boolean NOT NULL,
    changed_at timestamptz NOT NULL,
    confirmed_by_passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    PRIMARY KEY (installation_id,source_id,revision),
    UNIQUE (installation_id,operation_id)
);
CREATE TRIGGER sealed_source_serving BEFORE UPDATE OR DELETE ON relay_hosted.source_serving_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE VIEW relay_hosted.source_serving AS
    SELECT DISTINCT ON (installation_id,source_id) installation_id,source_id,revision,paused
    FROM relay_hosted.source_serving_changes ORDER BY installation_id,source_id,revision DESC;

-- Exclude paused supply from execution and live readiness, not pricing inputs.
CREATE OR REPLACE FUNCTION relay_hosted.text_compatible_supply(scope uuid,model_id text,inputs integer,outputs integer,
    multiplier integer,overhead integer,closure integer,fresh_only boolean)
RETURNS SETOF relay_hosted.text_supply_binding LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT s.* FROM (
        SELECT DISTINCT ON (i.source_id) i.* FROM relay_hosted.text_supply_binding i
        WHERE (i.scope_id,i.model,i.tokens_per_byte,i.overhead_tokens,i.close_after_s)
            = (scope,model_id,multiplier,overhead,closure)
            AND i.max_input_bytes>=inputs AND i.max_output_tokens>=outputs
        ORDER BY i.source_id,i.checked_at DESC,i.id
    ) s
    WHERE NOT fresh_only OR (least(s.valid_until,s.scope_expires_at)>statement_timestamp()
        AND NOT relay_hosted.text_inventory_fenced(s.id)
        AND NOT EXISTS(SELECT FROM relay_hosted.service_control c JOIN relay_hosted.source_serving v
            ON (v.installation_id,v.source_id)=(c.installation_id,s.source_id)
            WHERE c.qualification_scope_ref=s.scope_id AND v.paused))
$$;

-- New checks share the same fence as dispatch. A previously claimed check may
-- record its result; that evidence never grants work while the source is paused.
CREATE OR REPLACE FUNCTION relay_hosted.text_check_step(origin text,scope uuid,item uuid,source uuid,provider_rev bigint,proxy_rev bigint,
    proxied boolean,profile jsonb,step text)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; until_at timestamptz; job relay_hosted.text_check_attempts; profile_digest bytea; paused boolean;
BEGIN
    IF jsonb_typeof(profile)<>'object' OR octet_length(profile::text)>8192 OR step NOT IN ('prepare','claim')
        OR NOT profile @> jsonb_build_object('id',item,'scope',scope,'source',source,'provider_revision',provider_rev,
            'proxy_revision',proxy_rev,'uses_proxy',proxied) THEN RETURN 'denied'; END IF;
    profile_digest:=sha256(convert_to(profile::text,'UTF8'));
    SELECT h.id,q.expires_at INTO iid,until_at FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.id=scope FOR UPDATE OF s;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    IF iid IS NULL OR until_at<=clock_timestamp() OR NOT relay_hosted.text_source_matches(iid,source,provider_rev,proxy_rev,proxied)
        THEN RETURN 'denied'; END IF;
    -- The service row lock excludes a serving transition until this step commits.
    SELECT v.paused INTO paused FROM relay_hosted.source_serving v WHERE v.installation_id=iid AND v.source_id=source;
    SELECT * INTO job FROM relay_hosted.text_check_attempts WHERE id=item FOR UPDATE;
    IF FOUND THEN
        IF job.scope_id<>scope OR job.source_id<>source OR job.digest<>profile_digest THEN RETURN 'conflict'; END IF;
        IF EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item) THEN RETURN 'done'; END IF;
        IF paused THEN RETURN 'denied'; END IF;
        IF job.claimed OR job.expires_at<=clock_timestamp() THEN RETURN 'unconfirmed'; END IF;
        IF step='claim' THEN
            IF NOT pg_try_advisory_xact_lock(hashtextextended('text-source:'||source::text,0)) OR relay_hosted.text_probe_in_flight(source)
                OR relay_hosted.text_source_in_flight(source) THEN RETURN 'busy'; END IF;
            UPDATE relay_hosted.text_check_attempts SET claimed=true,expires_at=clock_timestamp()+interval '2 minutes' WHERE id=item;
            RETURN 'claimed';
        END IF;
        RETURN 'prepared';
    END IF;
    IF paused OR step<>'prepare' OR (SELECT count(*) FROM relay_hosted.text_check_attempts WHERE scope_id=scope)>=128
        THEN RETURN 'denied'; END IF;
    INSERT INTO relay_hosted.text_check_attempts VALUES(item,scope,source,profile_digest,least(until_at,clock_timestamp()+interval '2 minutes'),false);
    RETURN 'prepared';
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; page jsonb;
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    page:=relay_hosted.connection_page(iid,after_id);
    -- Keep the paired Desktop projection unchanged.
    RETURN jsonb_set(page,'{items}',(SELECT coalesce(jsonb_agg(item||jsonb_build_object(
        'serving',CASE WHEN item->>'credential_state'='stored' THEN jsonb_build_object(
            'revision',coalesce(v.revision,0)::text,
            'state',CASE WHEN NOT coalesce(v.paused,false) THEN 'enabled'
                WHEN relay_hosted.text_source_in_flight(source.id) OR relay_hosted.text_probe_in_flight(source.id)
                    THEN 'draining' ELSE 'paused' END) END)
        ORDER BY n),'[]'::jsonb)
        FROM jsonb_array_elements(page->'items') WITH ORDINALITY AS rows(item,n)
        CROSS JOIN LATERAL (SELECT (item->>'id')::uuid AS id) source
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(iid,source.id)));
END $$;

CREATE FUNCTION relay_hosted.change_source_serving(origin text,tag bytea,source uuid,op uuid,expected bigint,pause boolean,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; prior relay_hosted.source_serving_changes; v relay_hosted.source_serving;
    proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF source IS NULL OR source='00000000-0000-0000-0000-000000000000'
        OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 0 AND 9223372036854775806 OR pause IS NULL THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('source-serving-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes' THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.source_serving_changes c WHERE c.installation_id=iid AND c.operation_id=op;
    IF FOUND THEN RETURN CASE WHEN (prior.source_id,prior.revision-1,prior.actor,prior.paused)
        =(source,expected,ss.user_id,pause) THEN 'applied' ELSE 'conflict' END; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true THEN RETURN 'passkey_changed'; END IF;

    -- Same physical lock as the final dispatch permit, across models and aliases.
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    PERFORM FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=iid AND s.connection_id=source
        AND s.purpose='provider' AND s.generation IS NOT NULL FOR SHARE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO v FROM relay_hosted.source_serving s WHERE (s.installation_id,s.source_id)=(iid,source);
    IF coalesce(v.revision,0)<>expected OR coalesce(v.paused,false)=pause THEN RETURN 'conflict'; END IF;
    IF NOT pause AND (relay_hosted.text_source_in_flight(source) OR relay_hosted.text_probe_in_flight(source))
        THEN RETURN 'draining'; END IF;
    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.source_serving_changes(installation_id,source_id,revision,operation_id,actor,paused,changed_at,confirmed_by_passkey_id)
        VALUES(iid,source,expected+1,op,ss.user_id,pause,stamp,confirmed);
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.change_source_serving(text,bytea,uuid,uuid,bigint,boolean,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
