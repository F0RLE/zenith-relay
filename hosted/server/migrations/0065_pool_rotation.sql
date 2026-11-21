CREATE TABLE relay_hosted.text_pool_rotation_changes (
    operation_id uuid PRIMARY KEY,
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    revision bigint NOT NULL CHECK (revision>0),
    expected_revision text NOT NULL,
    mode text NOT NULL CHECK (mode IN ('automatic','failover')),
    member_order uuid[] NOT NULL CHECK (cardinality(member_order)<=128),
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    confirmed_by_passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    changed_at timestamptz NOT NULL,
    UNIQUE(scope_id,revision)
);
CREATE TRIGGER sealed_pool_rotation BEFORE UPDATE OR DELETE ON relay_hosted.text_pool_rotation_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.current_text_pool_rotation(scope uuid)
RETURNS relay_hosted.text_pool_rotation_changes LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT p FROM relay_hosted.text_pool_rotation_changes p WHERE p.scope_id=$1 ORDER BY p.revision DESC LIMIT 1
$$;

CREATE FUNCTION relay_hosted.text_pool_rotation_view(iid uuid,scope uuid,policy relay_hosted.text_pool_rotation_changes)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    WITH members AS (
        SELECT DISTINCT ON (i.source_id) i.source_id,i.source_label,coalesce(v.paused,false) AS paused,
            coalesce(v.revision,0) AS source_revision
        FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r USING(id)
        JOIN relay_hosted.connection_secret_slots cs
            ON (cs.installation_id,cs.connection_id,cs.purpose)=(iid,i.source_id,'provider') AND cs.generation IS NOT NULL
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(iid,i.source_id)
        WHERE i.scope_id=scope AND i.inventory_epoch=coalesce(v.inventory_epoch,0) AND r.admitted
            AND v.action IS DISTINCT FROM 'trash'
        ORDER BY i.source_id,i.checked_at DESC,i.id
        LIMIT 129
    ), ordered AS (
        SELECT count(*) AS total,
            coalesce(jsonb_agg(jsonb_build_object('source_id',source_id,'label',source_label,'paused',paused)
                ORDER BY array_position(policy.member_order,source_id) NULLS LAST,source_id),'[]') AS items,
            jsonb_agg(jsonb_build_array(source_id,source_revision) ORDER BY source_id) AS source_revisions
        FROM members
    )
    SELECT CASE WHEN total>128 THEN '{"error":"capacity"}'::jsonb ELSE jsonb_build_object(
        'scope_id',scope,'mode',coalesce(policy.mode,'automatic'),'members',items,
        'revision',encode(sha256(convert_to(jsonb_build_array(scope,policy.operation_id,items,source_revisions)::text,'UTF8')),'hex'))
    END FROM ordered
$$;

CREATE FUNCTION relay_hosted.owner_pool_rotation(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    RETURN coalesce((SELECT relay_hosted.text_pool_rotation_view(h.id,q.id,relay_hosted.current_text_pool_rotation(q.id))
        FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.expires_at>statement_timestamp()),
        '{"error":"missing"}'::jsonb);
END $$;

CREATE FUNCTION relay_hosted.change_pool_rotation(origin text,tag bytea,op uuid,scope uuid,expected text,
    strategy text,desired uuid[],proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,
    proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; sc relay_hosted.service_control; prior relay_hosted.text_pool_rotation_changes;
    snapshot jsonb; ord uuid[]; proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR scope IS NULL OR scope='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected COLLATE "C" !~ '^[0-9a-f]{64}$'
        OR strategy IS NULL OR strategy NOT IN ('automatic','failover') OR desired IS NULL
        OR cardinality(desired)>128 OR array_ndims(desired)>1 OR array_lower(desired,1)<>1
        OR (strategy='failover' AND cardinality(desired)=0)
        THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT s.* INTO STRICT sc FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        WHERE h.public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-rotation-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes' THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.text_pool_rotation_changes p WHERE p.operation_id=op;
    IF FOUND THEN RETURN CASE WHEN (prior.installation_id,prior.scope_id,prior.expected_revision,prior.actor,prior.mode,prior.member_order)
        =(sc.installation_id,scope,expected,ss.user_id,strategy,desired) THEN 'applied' ELSE 'conflict' END; END IF;
    IF sc.mode<>'qualification' OR sc.qualification_scope_ref IS DISTINCT FROM scope THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true THEN RETURN 'passkey_changed'; END IF;

    -- Serialize policy writes with holds. No provider, rule or wallet locks
    -- are taken here; the policy ranks only supply already allowed by a hold.
    IF proof_credential IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('text-pool-rotation:'||scope::text,0));
    END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.configuration_revisions q WHERE q.id=scope AND q.expires_at>clock_timestamp())
        THEN RETURN 'conflict'; END IF;
    prior:=relay_hosted.current_text_pool_rotation(scope);
    snapshot:=relay_hosted.text_pool_rotation_view(sc.installation_id,scope,prior);
    IF snapshot ? 'error' THEN RETURN snapshot->>'error'; END IF;
    IF snapshot->>'revision'<>expected THEN RETURN 'conflict'; END IF;
    SELECT ARRAY(SELECT (item->>'source_id')::uuid FROM jsonb_array_elements(snapshot->'members') item) INTO ord;
    IF ARRAY(SELECT id FROM unnest(desired) a(id) ORDER BY id)<>ARRAY(SELECT id FROM unnest(ord) a(id) ORDER BY id)
        OR array_position(desired,NULL) IS NOT NULL OR (strategy='automatic' AND desired<>ord) THEN RETURN 'invalid'; END IF;
    IF (strategy,desired)=(snapshot->>'mode',ord) THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.text_pool_rotation_changes(operation_id,installation_id,scope_id,revision,expected_revision,
        mode,member_order,actor,confirmed_by_passkey_id,changed_at)
        VALUES(op,sc.installation_id,scope,coalesce(prior.revision,0)+1,expected,strategy,desired,ss.user_id,confirmed,stamp);
    RETURN 'applied';
END $$;

ALTER TABLE relay_hosted.text_requests ADD COLUMN rotation_operation_id uuid REFERENCES relay_hosted.text_pool_rotation_changes;
CREATE FUNCTION relay_hosted.pin_text_pool_rotation()
RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE scope uuid;
BEGIN
    SELECT o.scope_id INTO STRICT scope FROM relay_hosted.text_offers o WHERE o.id=NEW.offer_id;
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('text-pool-rotation:'||scope::text,0));
    NEW.rotation_operation_id:=(relay_hosted.current_text_pool_rotation(scope)).operation_id;
    RETURN NEW;
END $$;
CREATE TRIGGER pin_text_pool_rotation BEFORE INSERT ON relay_hosted.text_requests
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.pin_text_pool_rotation();
CREATE TRIGGER immutable_text_rotation BEFORE UPDATE OF rotation_operation_id ON relay_hosted.text_requests
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE OR REPLACE FUNCTION relay_hosted.request_text_candidates(origin text,rid uuid,aid uuid,kid uuid,tag bytea)
RETURNS jsonb LANGUAGE sql VOLATILE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('scope',o.scope_id,'mode',coalesce(p.mode,'automatic'),'items',coalesce((
        SELECT jsonb_agg(jsonb_build_object('item',i.id,'source',i.source_id,'model',i.model,'endpoint',i.base_url,
            'provider_revision',i.provider_revision,'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,
            'direct_ipv4',host(i.direct_ipv4),'input_bytes',o.max_input_bytes,
            'rate_revision',i.rate_revision,'cost',v.cost,'busy',a.state='busy','priority',i.priority)
            ORDER BY i.id)
        FROM (
            SELECT b.*,CASE WHEN p.mode='failover' THEN
                -(row_number() OVER (ORDER BY array_position(p.member_order,b.source_id) NULLS LAST,b.source_id))::integer
                ELSE 0 END AS priority
            FROM relay_hosted.text_offer_supply(o) b WHERE b.id=ANY(coalesce(r.supply_items,ARRAY[o.inventory_id]))
        ) i
        CROSS JOIN LATERAL (SELECT relay_hosted.text_cost_bound(r.input_bound,r.output_bound,i.input_rate,i.cached_rate,i.output_rate) cost) v
        CROSS JOIN LATERAL (SELECT relay_hosted.text_source_state(i.source_id,i.model,v.cost,clock_timestamp()) state) a
        WHERE a.state<>'blocked'
    ),'[]'::jsonb))
    FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
    LEFT JOIN relay_hosted.text_pool_rotation_changes p ON (p.operation_id,p.scope_id)=(r.rotation_operation_id,o.scope_id)
    JOIN relay_hosted.text_model_rules rule ON rule.id=o.inventory_id AND rule.enabled AND o.revision>=rule.first_live_revision
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id AND s.admission_epoch=r.authority_epoch
    JOIN relay_hosted.customer_keys k ON k.id=r.key_id
    WHERE (r.id,r.attempt_id,r.key_id,r.send_tag)=($2,$3,$4,$5) AND r.execution='reserved' AND r.claim_by>clock_timestamp()
        AND relay_hosted.qualified_key_workspace($1,k.id,k.verifier)=r.workspace_id
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_pool_activity(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope uuid; policy relay_hosted.text_pool_rotation_changes;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    SELECT s.qualification_scope_ref INTO STRICT scope
        FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
        WHERE h.public_origin=origin;
    policy:=relay_hosted.current_text_pool_rotation(scope);
    RETURN jsonb_build_object(
        'strategy',CASE WHEN scope IS NOT NULL THEN coalesce(policy.mode,'automatic') END,
        'member_order',CASE WHEN policy.mode='failover' THEN policy.member_order ELSE '{}'::uuid[] END,
        'items',coalesce((
            SELECT jsonb_agg(jsonb_build_object('source_id',a.source_id,'source',a.label,'model',a.model,'requests',a.requests)
                ORDER BY a.source_id,a.model)
            FROM (
                SELECT i.source_id,min(i.source_label) AS label,o.model,count(*) AS requests
                FROM relay_hosted.text_offers o JOIN relay_hosted.text_requests r ON r.offer_id=o.id
                JOIN relay_hosted.text_purchase_attempts p ON p.attempt_id=r.attempt_id
                JOIN relay_hosted.text_inventory i ON (i.id,i.scope_id)=(p.item,o.scope_id)
                WHERE o.scope_id=scope AND r.execution='dispatched' AND r.closed_at IS NULL
                    AND r.claimed_at IS NOT NULL AND r.close_by>statement_timestamp()
                GROUP BY i.source_id,o.model
            ) a
        ),'[]'::jsonb)
    );
END $$;

REVOKE ALL ON FUNCTION relay_hosted.current_text_pool_rotation(uuid),
    relay_hosted.text_pool_rotation_view(uuid,uuid,relay_hosted.text_pool_rotation_changes),
    relay_hosted.owner_pool_rotation(text,bytea),relay_hosted.pin_text_pool_rotation(),
    relay_hosted.change_pool_rotation(text,bytea,uuid,uuid,text,text,uuid[],bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
