-- A block belongs to the saved source and exact model, not to one check,
-- price revision or qualification scope.
CREATE TABLE relay_hosted.source_model_blocks (
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    source_id uuid NOT NULL,
    model text NOT NULL CHECK (model COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$'),
    PRIMARY KEY (installation_id,source_id,model)
);
CREATE TABLE relay_hosted.source_model_policy_changes (
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    operation_id uuid NOT NULL,
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    source_revision bigint NOT NULL CHECK (source_revision>=0),
    changes jsonb NOT NULL,
    changed_at timestamptz NOT NULL,
    confirmed_by_passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    PRIMARY KEY (installation_id,source_id,revision),
    UNIQUE (installation_id,operation_id)
);
CREATE TRIGGER sealed_source_model_policy BEFORE UPDATE OR DELETE ON relay_hosted.source_model_policy_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

-- Both the reserved candidate list and the authoritative read under the
-- physical-source lock use this gate. Historical purchase inputs do not.
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
        AND NOT EXISTS(SELECT FROM relay_hosted.service_control c
            LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(c.installation_id,s.source_id)
            LEFT JOIN relay_hosted.source_model_blocks b
                ON (b.installation_id,b.source_id,b.model)=(c.installation_id,s.source_id,s.model)
            WHERE c.qualification_scope_ref=s.scope_id AND (v.paused OR b.model IS NOT NULL)))
$$;

CREATE FUNCTION relay_hosted.owner_member_models(origin text,tag bytea,source uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    RETURN coalesce((SELECT jsonb_build_object(
        'scope_id',s.qualification_scope_ref,'source_revision',coalesce(v.revision,0)::text,
        'revision',coalesce((SELECT max(p.revision) FROM relay_hosted.source_model_policy_changes p
            WHERE (p.installation_id,p.source_id)=(h.id,source)),0)::text,
        'items',models.items)
        FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.connection_secret_slots cs
            ON (cs.installation_id,cs.connection_id,cs.purpose)=(h.id,source,'provider') AND cs.generation IS NOT NULL
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(h.id,source)
        CROSS JOIN LATERAL (
            SELECT jsonb_agg(jsonb_build_object('model',m.model,'allowed',b.model IS NULL)
                ORDER BY m.model COLLATE "C") AS items
            FROM (
                SELECT DISTINCT i.model FROM relay_hosted.text_inventory i
                JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.admitted
                WHERE (i.scope_id,i.source_id,i.inventory_epoch)=(s.qualification_scope_ref,source,coalesce(v.inventory_epoch,0))
            ) m LEFT JOIN relay_hosted.source_model_blocks b
                ON (b.installation_id,b.source_id,b.model)=(h.id,source,m.model)
        ) models
        WHERE h.public_origin=origin AND s.mode='qualification' AND models.items IS NOT NULL
            AND v.action IS DISTINCT FROM 'trash'), '{"error":"missing"}'::jsonb);
END $$;

CREATE FUNCTION relay_hosted.change_member_models(origin text,tag bytea,source uuid,op uuid,scope uuid,lifecycle_rev bigint,
    expected bigint,delta jsonb,proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,
    proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; sc relay_hosted.service_control; v relay_hosted.source_serving;
    prior relay_hosted.source_model_policy_changes; model_id text; epoch bigint; confirmed uuid; stamp timestamptz;
    proof_result text;
BEGIN
    IF source IS NULL OR source='00000000-0000-0000-0000-000000000000'
        OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR scope IS NULL OR scope='00000000-0000-0000-0000-000000000000'
        OR lifecycle_rev IS NULL OR lifecycle_rev<0 OR expected IS NULL OR expected NOT BETWEEN 0 AND 9223372036854775806
        OR jsonb_typeof(delta) IS DISTINCT FROM 'object' OR delta='{}'::jsonb OR octet_length(delta::text)>24576
        THEN RETURN 'invalid'; END IF;
    IF (SELECT count(*) FROM jsonb_each(delta))>128 OR EXISTS(SELECT FROM jsonb_each(delta) d
        WHERE d.key COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$' OR jsonb_typeof(d.value)<>'boolean')
        THEN RETURN 'invalid'; END IF;

    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT s.* INTO STRICT sc FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
        WHERE h.public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('source-model-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes' THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.source_model_policy_changes p
        WHERE p.installation_id=sc.installation_id AND p.operation_id=op;
    IF FOUND THEN RETURN CASE WHEN (prior.source_id,prior.scope_id,prior.source_revision,prior.revision-1,prior.actor,prior.changes)
        =(source,scope,lifecycle_rev,expected,ss.user_id,delta) THEN 'applied' ELSE 'conflict' END; END IF;
    IF sc.mode<>'qualification' OR sc.qualification_scope_ref IS DISTINCT FROM scope THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true
        THEN RETURN 'passkey_changed'; END IF;

    -- Match lifecycle lock order. Admission/publication cannot overtake the
    -- edit, and we never write rule rows held by a waiting dispatch claim.
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    FOR model_id IN SELECT key FROM jsonb_each(delta) ORDER BY key LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||scope::text||':'||model_id,0));
    END LOOP;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    PERFORM FROM relay_hosted.connection_secret_slots cs WHERE (cs.installation_id,cs.connection_id,cs.purpose)
        =(sc.installation_id,source,'provider') AND cs.generation IS NOT NULL FOR SHARE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO v FROM relay_hosted.source_serving l WHERE (l.installation_id,l.source_id)=(sc.installation_id,source);
    epoch:=coalesce(v.inventory_epoch,0);
    IF coalesce(v.revision,0)<>lifecycle_rev OR v.action='trash' OR expected<>coalesce(
        (SELECT max(p.revision) FROM relay_hosted.source_model_policy_changes p
            WHERE (p.installation_id,p.source_id)=(sc.installation_id,source)),0) THEN RETURN 'conflict'; END IF;
    IF EXISTS(SELECT FROM jsonb_each(delta) d
        LEFT JOIN relay_hosted.source_model_blocks b
            ON (b.installation_id,b.source_id,b.model)=(sc.installation_id,source,d.key)
        WHERE (d.value='true'::jsonb)=(b.model IS NULL) OR NOT EXISTS(
            SELECT FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.admitted
            WHERE (i.scope_id,i.source_id,i.inventory_epoch,i.model)=(scope,source,epoch,d.key)))
        THEN RETURN 'conflict'; END IF;

    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;

    INSERT INTO relay_hosted.source_model_policy_changes(installation_id,source_id,revision,operation_id,actor,scope_id,
        source_revision,changes,changed_at,confirmed_by_passkey_id)
        VALUES(sc.installation_id,source,expected+1,op,ss.user_id,scope,lifecycle_rev,delta,stamp,confirmed);
    DELETE FROM relay_hosted.source_model_blocks b
        WHERE (b.installation_id,b.source_id)=(sc.installation_id,source) AND delta->b.model='true'::jsonb;
    INSERT INTO relay_hosted.source_model_blocks(installation_id,source_id,model)
        SELECT sc.installation_id,source,d.key FROM jsonb_each(delta) d WHERE d.value='false'::jsonb;
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_member_models(text,bytea,uuid),
    relay_hosted.change_member_models(text,bytea,uuid,uuid,uuid,bigint,bigint,jsonb,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
