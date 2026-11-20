-- An offer fixes the customer tariff and sizing, not the physical supplier.
-- Requests written before this migration retain their original single route.
ALTER TABLE relay_hosted.text_requests ADD COLUMN supply_items uuid[]
    CHECK (supply_items IS NULL OR (cardinality(supply_items) BETWEEN 1 AND 128
        AND array_position(supply_items,NULL::uuid) IS NULL));
CREATE TRIGGER immutable_text_supply BEFORE UPDATE OF supply_items ON relay_hosted.text_requests
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE INDEX text_active_send ON relay_hosted.text_requests(attempt_id)
    WHERE closed_at IS NULL AND execution IN ('dispatched','uncertain');

CREATE TABLE relay_hosted.text_route_fences (
    item uuid PRIMARY KEY REFERENCES relay_hosted.text_inventory,
    attempt uuid NOT NULL REFERENCES relay_hosted.text_purchase_attempts,
    at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER sealed_route_fence BEFORE UPDATE OR DELETE ON relay_hosted.text_route_fences
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TABLE relay_hosted.text_route_waits (
    source_id uuid NOT NULL,
    model text COLLATE "C" NOT NULL,
    not_before timestamptz NOT NULL,
    PRIMARY KEY (source_id,model)
);

CREATE FUNCTION relay_hosted.text_inventory_fenced(item uuid)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.text_route_fences f WHERE f.item=$1)
        OR EXISTS(SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers o ON o.id=f.offer_id
            WHERE o.inventory_id=$1)
$$;

CREATE VIEW relay_hosted.text_supply_binding AS
    SELECT i.*,q.expires_at AS scope_expires_at,p.revision AS rate_revision,p.input_rate,p.cached_rate,p.output_rate
    FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.admitted
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id AND s.mode='qualification'
    JOIN relay_hosted.configuration_revisions q ON q.id=i.scope_id
    LEFT JOIN relay_hosted.text_purchase_rates p ON (p.item,p.revision)=(i.id,r.purchase_revision)
    WHERE relay_hosted.text_source_matches(s.installation_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy);

-- Quote and dispatch use the same sizing contract. A newer admitted check
-- replaces older compatible evidence on the same binding, even if unpriced.
CREATE FUNCTION relay_hosted.text_compatible_supply(scope uuid,model_id text,inputs integer,outputs integer,
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
        AND NOT relay_hosted.text_inventory_fenced(s.id))
$$;

CREATE FUNCTION relay_hosted.text_offer_supply(o relay_hosted.text_offers)
RETURNS SETOF relay_hosted.text_supply_binding LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT i.* FROM relay_hosted.text_compatible_supply(o.scope_id,o.model,o.max_input_bytes,o.max_output_tokens,
        o.tokens_per_byte,o.overhead_tokens,o.close_after_s,true) i WHERE i.rate_revision IS NOT NULL
$$;

CREATE OR REPLACE VIEW relay_hosted.active_text_listings AS
    SELECT h.public_origin,r.id AS item,o.*
    FROM relay_hosted.text_model_rules r JOIN relay_hosted.text_offers o ON o.id=r.last_offer AND o.inventory_id=r.id
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id AND s.mode='qualification'
    JOIN relay_hosted.installations h ON h.id=s.installation_id
    WHERE r.enabled AND r.admitted AND EXISTS(SELECT FROM relay_hosted.text_offer_supply(o));

-- Publication and customer authorization do not depend on the anchor's credential.
-- Each selected supplier is rechecked separately under its physical-source lock.
CREATE OR REPLACE FUNCTION relay_hosted.lock_text_authority(origin text,oid uuid,kid uuid,proof bytea)
RETURNS uuid LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE space uuid; until_at timestamptz;
BEGIN
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT k.workspace_id,least(k.expires_at,q.expires_at) INTO space,until_at
    FROM relay_hosted.customer_keys k JOIN relay_hosted.configuration_revisions q ON q.id=k.scope_id
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=q.id
    JOIN relay_hosted.installations h ON h.id=s.installation_id
    JOIN relay_hosted.text_offers o ON o.id=oid AND o.scope_id=q.id
    JOIN relay_hosted.text_model_rules r ON r.id=o.inventory_id AND o.revision>=r.first_live_revision
    JOIN relay_hosted.workspaces w ON w.id=k.workspace_id
    JOIN relay_hosted.qualification_subjects a ON a.scope_id=q.id AND a.workspace_id=w.id
    JOIN relay_hosted.external_identities e ON e.provider_id=q.provider_id AND e.subject_digest=a.subject_digest
    JOIN relay_hosted.users u ON u.id=e.user_id AND u.id=w.owner_user_id
    WHERE h.public_origin=origin AND s.mode='qualification' AND r.admitted AND r.enabled AND k.id=kid AND k.verifier=proof
        AND k.installation_epoch=h.authorization_epoch AND k.user_epoch=u.authorization_epoch AND k.revoked_at IS NULL
        AND e.revoked_at IS NULL AND u.disabled_at IS NULL
    FOR SHARE OF k,q,s,h,r,w,a,e,u;
    IF NOT FOUND OR until_at<=clock_timestamp() THEN RETURN NULL; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.text_offers o CROSS JOIN LATERAL relay_hosted.text_offer_supply(o) p
        WHERE o.id=oid) THEN RETURN NULL; END IF;
    RETURN space;
END $$;

CREATE FUNCTION relay_hosted.text_cost_bound(sized integer,output_cap integer,input_rate numeric,cached_rate numeric,output_rate numeric)
RETURNS bigint LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT ceil(sized*greatest(input_rate,cached_rate)+output_cap*output_rate)::bigint
$$;

CREATE OR REPLACE FUNCTION relay_hosted.reserve_text(origin text,oid uuid,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; r relay_hosted.text_requests; balance bigint; stamp timestamptz; sized integer; supply uuid[];
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid;
    space:=relay_hosted.qualified_key_workspace(origin,kid,proof);
    IF space IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT w.available_micro_usd INTO STRICT balance FROM relay_hosted.wallets w WHERE w.workspace_id=space FOR NO KEY UPDATE;
    IF relay_hosted.qualified_key_workspace(origin,kid,proof) IS DISTINCT FROM space THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    IF op IS NOT NULL THEN
        SELECT * INTO r FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.key_id=kid AND x.operation_tag=op;
        IF FOUND THEN
            IF r.input_tag IS DISTINCT FROM body_tag THEN result:='conflict';
            ELSE rid:=r.id; result:=CASE WHEN r.closed_at IS NULL THEN 'in_progress' ELSE 'finished' END; END IF; space:=NULL;
            RETURN NEXT; RETURN;
        END IF;
    END IF;
    IF o.id IS NULL OR requested_model IS DISTINCT FROM o.model OR input_bytes IS NULL OR output_cap IS NULL
        OR input_bytes NOT BETWEEN 1 AND o.max_input_bytes OR output_cap NOT BETWEEN 1 AND o.max_output_tokens THEN
        result:='unoffered'; RETURN NEXT; RETURN; END IF;
    IF relay_hosted.lock_text_authority(origin,oid,kid,proof) IS DISTINCT FROM space THEN
        result:='unoffered'; RETURN NEXT; RETURN; END IF;
    SELECT array_agg(p.id ORDER BY p.id) INTO supply FROM relay_hosted.text_offer_supply(o) p;
    IF supply IS NULL THEN result:='unoffered'; RETURN NEXT; RETURN; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.closed_at IS NULL)>=20 THEN
        result:='capacity'; RETURN NEXT; RETURN; END IF;
    SELECT s.admission_epoch INTO epoch FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id;
    sized:=input_bytes*o.tokens_per_byte+o.overhead_tokens;
    held:=relay_hosted.text_cost_bound(sized,output_cap,o.input_rate,o.cached_rate,o.output_rate);
    IF balance<held THEN result:='funds'; RETURN NEXT; RETURN; END IF;
    stamp:=clock_timestamp();
    INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,operation_tag,input_tag,send_tag,
        input_bound,output_bound,held,started_at,claim_by,close_by,supply_items)
    VALUES(space,kid,oid,epoch,op,body_tag,claim_tag,sized,output_cap,held,stamp,
        stamp+interval '30 seconds',stamp+o.close_after_s*interval '1 second',supply) RETURNING * INTO r;
    UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd-r.held,reserved_micro_usd=w.reserved_micro_usd+r.held,
        revision=w.revision+1 WHERE w.workspace_id=space;
    PERFORM relay_hosted.record_request_journal(r,'request_hold');
    result:='new'; rid:=r.id; attempt:=r.attempt_id; RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.text_source_in_flight(src uuid)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS (
        SELECT FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        LEFT JOIN relay_hosted.text_purchase_attempts a USING(attempt_id)
        LEFT JOIN relay_hosted.text_inventory i ON i.id=a.item
        WHERE r.closed_at IS NULL AND r.execution IN ('dispatched','uncertain')
            AND coalesce(i.source_id,o.source_id)=src
    )
$$;

CREATE FUNCTION relay_hosted.text_source_state(src uuid,model_id text,cost bigint,stamp timestamptz)
RETURNS text LANGUAGE sql VOLATILE STRICT SET search_path=pg_catalog AS $$
    SELECT CASE
    WHEN EXISTS(SELECT FROM relay_hosted.text_route_waits w WHERE (w.source_id,w.model)=(src,model_id) AND w.not_before>stamp)
        THEN 'blocked'
    WHEN relay_hosted.text_probe_in_flight(src) OR relay_hosted.text_source_in_flight(src) THEN 'busy'
    WHEN EXISTS (
        SELECT FROM (SELECT c.limit_micro_usd FROM relay_hosted.text_purchase_limit_changes c
            WHERE c.source_id=src ORDER BY c.revision DESC LIMIT 1) cap
        LEFT JOIN relay_hosted.text_purchase_periods m ON m.source_id=src
            AND m.month_utc=date_trunc('month',stamp AT TIME ZONE 'UTC')::date
        WHERE cap.limit_micro_usd IS NOT NULL AND (
            coalesce(m.unpriced,false) OR coalesce(m.accounted_micro_usd+m.outstanding_micro_usd,0)+cost>cap.limit_micro_usd)
    ) THEN 'blocked'
    ELSE 'ready' END
$$;

CREATE FUNCTION relay_hosted.request_text_candidates(origin text,rid uuid,aid uuid,kid uuid,tag bytea)
RETURNS jsonb LANGUAGE sql VOLATILE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('scope',o.scope_id,'items',coalesce((
        SELECT jsonb_agg(jsonb_build_object('item',i.id,'source',i.source_id,'model',i.model,'endpoint',i.base_url,
            'provider_revision',i.provider_revision,'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,
            'direct_ipv4',host(i.direct_ipv4),'input_bytes',o.max_input_bytes,
            'rate_revision',i.rate_revision,'cost',v.cost,'busy',a.state='busy') ORDER BY i.id)
        FROM relay_hosted.text_offer_supply(o) i
        CROSS JOIN LATERAL (SELECT relay_hosted.text_cost_bound(r.input_bound,r.output_bound,i.input_rate,i.cached_rate,i.output_rate) cost) v
        CROSS JOIN LATERAL (SELECT relay_hosted.text_source_state(i.source_id,i.model,v.cost,clock_timestamp()) state) a
        WHERE i.id=ANY(coalesce(r.supply_items,ARRAY[o.inventory_id]))
            AND a.state<>'blocked'
    ),'[]'::jsonb))
    FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
    JOIN relay_hosted.text_model_rules rule ON rule.id=o.inventory_id AND rule.enabled AND o.revision>=rule.first_live_revision
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id AND s.admission_epoch=r.authority_epoch
    JOIN relay_hosted.customer_keys k ON k.id=r.key_id
    WHERE (r.id,r.attempt_id,r.key_id,r.send_tag)=($2,$3,$4,$5) AND r.execution='reserved' AND r.claim_by>clock_timestamp()
        AND relay_hosted.qualified_key_workspace($1,k.id,k.verifier)=r.workspace_id
$$;

CREATE FUNCTION relay_hosted.claim_text_candidate(origin text,rid uuid,aid uuid,space uuid,kid uuid,src uuid,
    model text,auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer,
    inventory uuid,purchase_rev bigint)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE proof bytea; o relay_hosted.text_offers; req relay_hosted.text_requests; p relay_hosted.text_supply_binding;
    stamp timestamptz; key_expires timestamptz; purchase_bound bigint; state text;
BEGIN
    -- Neither the caller's rank nor the candidate read is permission to send.
    SELECT x.* INTO o FROM relay_hosted.text_offers x JOIN relay_hosted.text_requests r ON r.offer_id=x.id
        WHERE r.id=rid AND r.attempt_id=aid AND r.workspace_id=space AND r.key_id=kid;
    IF NOT FOUND OR (o.model,o.revision,o.max_input_bytes)<>(model,tariff_rev,input_cap) THEN RETURN 'denied'; END IF;
    SELECT k.verifier,k.expires_at INTO proof,key_expires FROM relay_hosted.customer_keys k WHERE k.id=kid AND k.workspace_id=space;
    IF proof IS NULL OR relay_hosted.lock_text_authority(origin,o.id,kid,proof) IS DISTINCT FROM space
        OR NOT EXISTS(SELECT FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id AND s.admission_epoch=auth_rev)
        THEN RETURN 'denied'; END IF;
    -- Lock the current purchase pointer so a cheaper stale quote cannot authorize a send.
    PERFORM FROM relay_hosted.text_model_rules r WHERE r.id=inventory AND r.admitted AND r.purchase_revision=purchase_rev FOR SHARE;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT * INTO req FROM relay_hosted.text_requests r
        WHERE (r.id,r.attempt_id,r.workspace_id,r.key_id,r.offer_id,r.held,r.send_tag,r.output_bound)
            =(rid,aid,space,kid,o.id,bound,tag,output_cap) AND r.authority_epoch=auth_rev AND r.execution='reserved'
            AND inventory=ANY(coalesce(r.supply_items,ARRAY[o.inventory_id])) FOR UPDATE;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||src::text,0));
    SELECT * INTO p FROM relay_hosted.text_offer_supply(o) x
        WHERE x.id=inventory AND x.source_id=src AND x.rate_revision=purchase_rev;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    stamp:=clock_timestamp();
    IF req.claim_by<=stamp OR least(p.valid_until,p.scope_expires_at,key_expires)<=stamp
        OR relay_hosted.qualified_key_workspace(origin,kid,proof) IS DISTINCT FROM space THEN RETURN 'denied'; END IF;
    purchase_bound:=relay_hosted.text_cost_bound(req.input_bound,req.output_bound,p.input_rate,p.cached_rate,p.output_rate);
    state:=relay_hosted.text_source_state(src,p.model,purchase_bound,stamp);
    IF state='busy' THEN RETURN 'busy'; END IF;
    IF state IS DISTINCT FROM 'ready' THEN RETURN 'denied'; END IF;
    UPDATE relay_hosted.text_requests SET execution='dispatched',claimed_at=stamp WHERE id=rid;
    INSERT INTO relay_hosted.text_purchase_attempts(attempt_id,item,rate_revision,month_utc,bound_micro_usd)
        VALUES(aid,p.id,p.rate_revision,date_trunc('month',stamp AT TIME ZONE 'UTC')::date,purchase_bound);
    RETURN 'claimed';
END $$;

-- Kept private for existing single-route migration/settlement fixtures.
CREATE OR REPLACE FUNCTION relay_hosted.claim_text(origin text,oid uuid,rid uuid,aid uuid,space uuid,kid uuid,source uuid,model text,
    auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer)
RETURNS boolean LANGUAGE sql VOLATILE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT coalesce((SELECT relay_hosted.claim_text_candidate($1,$3,$4,$5,$6,$7,$8,$9,$10,
        $11,$12,$13,$14,r.id,r.purchase_revision)
        FROM relay_hosted.text_requests x JOIN relay_hosted.text_offers o ON o.id=x.offer_id
        JOIN relay_hosted.text_model_rules r ON r.id=o.inventory_id WHERE x.id=$3 AND o.id=$2 AND o.source_id=$7)='claimed',false)
$$;

CREATE FUNCTION relay_hosted.record_text_cooldown(oid uuid,rid uuid,aid uuid,delay_ms bigint)
RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE model_id text; src uuid; deadline timestamptz;
BEGIN
    IF delay_ms<1 THEN RETURN; END IF;
    SELECT i.model,i.source_id INTO model_id,src FROM relay_hosted.text_requests r
        JOIN relay_hosted.text_purchase_attempts a USING(attempt_id) JOIN relay_hosted.text_inventory i ON i.id=a.item
        WHERE r.id=rid AND r.offer_id=oid AND a.attempt_id=aid AND r.closed_at IS NULL;
    IF NOT FOUND THEN RETURN; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||src::text,0));
    BEGIN
        deadline:=clock_timestamp()+delay_ms*interval '1 millisecond';
    EXCEPTION WHEN datetime_field_overflow OR interval_field_overflow OR numeric_value_out_of_range THEN
        -- An unrepresentable delay must not reopen the route early.
        deadline:='infinity';
    END;
    INSERT INTO relay_hosted.text_route_waits AS w(source_id,model,not_before)
        VALUES(src,model_id,deadline)
    ON CONFLICT(source_id,model) DO UPDATE SET not_before=greatest(w.not_before,excluded.not_before);
END $$;

CREATE FUNCTION relay_hosted.fence_text_route(oid uuid,aid uuid)
RETURNS void LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE route uuid; src uuid;
BEGIN
    SELECT a.item,i.source_id INTO route,src FROM relay_hosted.text_purchase_attempts a
        JOIN relay_hosted.text_inventory i ON i.id=a.item WHERE a.attempt_id=aid;
    IF NOT FOUND THEN
        INSERT INTO relay_hosted.text_fences(offer_id) VALUES(oid) ON CONFLICT DO NOTHING;
        RETURN;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||src::text,0));
    INSERT INTO relay_hosted.text_route_fences(item,attempt) VALUES(route,aid) ON CONFLICT DO NOTHING;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.value_text_purchase(oid uuid,aid uuid,inputs bigint,cached bigint,outputs bigint)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE owed numeric; ceiling bigint;
BEGIN
    SELECT ceil((inputs-coalesce(cached,0))*p.input_rate+coalesce(cached,0)*p.cached_rate+outputs*p.output_rate),a.bound_micro_usd
        INTO owed,ceiling FROM relay_hosted.text_purchase_attempts a JOIN relay_hosted.text_purchase_rates p
        ON (p.item,p.revision)=(a.item,a.rate_revision)
        WHERE a.attempt_id=aid AND inputs>=0 AND outputs>=0 AND (cached BETWEEN 0 AND inputs OR
            (cached IS NULL AND (inputs=0 OR p.input_rate=p.cached_rate)));
    IF NOT FOUND THEN RETURN; END IF;
    INSERT INTO relay_hosted.text_purchase_results(attempt_id,input_tokens,cached_tokens,output_tokens,valued_micro_usd)
        VALUES(aid,inputs,cached,outputs,owed) ON CONFLICT (attempt_id) DO NOTHING;
    IF FOUND AND owed>ceiling THEN PERFORM relay_hosted.fence_text_route(oid,aid); END IF;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.close_text(r relay_hosted.text_requests,next_execution text,evidence text)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; value numeric; debit bigint; held_amount bigint:=r.held;
BEGIN
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=r.offer_id;
    value:=CASE WHEN evidence IN ('provider_final','provider_partial') THEN ceil(
        CASE WHEN r.input_tokens IS NULL THEN 0 WHEN r.cached_tokens IS NULL THEN
            CASE WHEN o.input_rate=o.cached_rate THEN r.input_tokens*o.input_rate ELSE 0 END
        ELSE (r.input_tokens-r.cached_tokens)*o.input_rate+r.cached_tokens*o.cached_rate END
        +coalesce(r.output_tokens,0)*o.output_rate) ELSE NULL END;
    debit:=least(held_amount,coalesce(value,0))::bigint;
    IF value>held_amount OR r.input_tokens>r.input_bound OR r.output_tokens>r.output_bound THEN
        PERFORM relay_hosted.fence_text_route(r.offer_id,r.attempt_id);
    END IF;
    UPDATE relay_hosted.text_requests SET execution=next_execution,accounting=CASE WHEN value IS NULL THEN 'released' ELSE 'settled' END,
        basis=evidence,valued=value,charged=debit,closed_at=clock_timestamp() WHERE id=r.id AND closed_at IS NULL RETURNING * INTO STRICT r;
    UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd+(held_amount-debit),reserved_micro_usd=w.reserved_micro_usd-held_amount,
        revision=w.revision+1 WHERE w.workspace_id=r.workspace_id AND reserved_micro_usd>=held_amount;
    IF NOT FOUND THEN RAISE EXCEPTION 'reservation projection mismatch'; END IF;
    PERFORM relay_hosted.record_request_journal(r,'request_close');
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.text_check_step(origin text,scope uuid,item uuid,source uuid,provider_rev bigint,proxy_rev bigint,
    proxied boolean,profile jsonb,step text)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; until_at timestamptz; job relay_hosted.text_check_attempts; profile_digest bytea;
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
    SELECT * INTO job FROM relay_hosted.text_check_attempts WHERE id=item FOR UPDATE;
    IF FOUND THEN
        IF job.scope_id<>scope OR job.source_id<>source OR job.digest<>profile_digest THEN RETURN 'conflict'; END IF;
        IF EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item) THEN RETURN 'done'; END IF;
        IF job.claimed OR job.expires_at<=clock_timestamp() THEN RETURN 'unconfirmed'; END IF;
        IF step='claim' THEN
            IF NOT pg_try_advisory_xact_lock(hashtextextended('text-source:'||source::text,0)) OR relay_hosted.text_probe_in_flight(source)
                OR relay_hosted.text_source_in_flight(source) THEN RETURN 'busy'; END IF;
            UPDATE relay_hosted.text_check_attempts SET claimed=true,expires_at=clock_timestamp()+interval '2 minutes' WHERE id=item;
            RETURN 'claimed';
        END IF;
        RETURN 'prepared';
    END IF;
    IF step<>'prepare' OR (SELECT count(*) FROM relay_hosted.text_check_attempts WHERE scope_id=scope)>=128 THEN RETURN 'denied'; END IF;
    INSERT INTO relay_hosted.text_check_attempts VALUES(item,scope,source,profile_digest,least(until_at,clock_timestamp()+interval '2 minutes'),false);
    RETURN 'prepared';
END $$;

CREATE FUNCTION relay_hosted.text_basis_supply(i relay_hosted.text_inventory,fresh_only boolean)
RETURNS SETOF relay_hosted.text_supply_binding LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT * FROM relay_hosted.text_compatible_supply(i.scope_id,i.model,i.max_input_bytes,i.max_output_tokens,
        i.tokens_per_byte,i.overhead_tokens,i.close_after_s,fresh_only)
$$;

-- Price review uses compatible admitted bindings, independent of transient availability.
CREATE FUNCTION relay_hosted.text_price_inputs(i relay_hosted.text_inventory,inherit boolean)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object('source',p.source_id,'item',p.id,
        'purchase_revision',p.rate_revision,'rates',CASE WHEN p.rate_revision IS NOT NULL THEN
            jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,'output',p.output_rate::text) END,
        'default_revision',m.revision,'default_percent',m.percent::text) ORDER BY p.source_id),'[]')
    FROM relay_hosted.text_basis_supply(i,false) p
    LEFT JOIN relay_hosted.latest_text_source_markup m ON inherit AND m.source_id=p.source_id
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_text_price_inputs(origin text,tag bytea,item_id uuid,inherit boolean)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN coalesce((SELECT relay_hosted.text_price_inputs(i,inherit)
        FROM relay_hosted.text_inventory i JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        JOIN relay_hosted.installations h ON h.id=s.installation_id WHERE i.id=item_id AND h.public_origin=origin),'[]');
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,'source',i.source_label,'proxy',i.uses_proxy,
        'revision',r.revision::text,'admitted',r.admitted,'enabled',r.enabled,
        'ready',c.id IS NOT NULL AND NOT relay_hosted.text_inventory_fenced(i.id),
        'supply_ready',EXISTS(SELECT FROM relay_hosted.text_basis_supply(i,true) p WHERE p.rate_revision IS NOT NULL),
        'purchase',CASE WHEN p.item IS NOT NULL THEN jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,
            'output',p.output_rate::text) END,
        'source_markup',jsonb_build_object('revision',coalesce(m.revision,0)::text,'percent',m.percent::text),
        'sale_rule',jsonb_build_object('revision',coalesce(q.revision,0)::text,'mode',coalesce(q.mode,'fixed'),
            'percent',q.percent::text,'stale',q.inputs IS NOT NULL AND q.inputs<>relay_hosted.text_price_inputs(i,q.mode='api_default'),
            'below_cost',EXISTS(SELECT FROM relay_hosted.text_basis_supply(i,false) cost WHERE
                (draft.rates->>'input')::numeric<cost.input_rate OR (draft.rates->>'cached')::numeric<cost.cached_rate
                OR (draft.rates->>'output')::numeric<cost.output_rate)),
        'draft',draft.rates,'effective',CASE WHEN l.id IS NOT NULL THEN relay_hosted.text_rates(o) END)
        ORDER BY i.model,i.source_label,i.id),'[]') FROM relay_hosted.text_inventory i
        JOIN relay_hosted.text_model_rules r ON r.id=i.id JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        JOIN relay_hosted.installations h ON h.id=s.installation_id AND h.public_origin=origin
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id
        LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=r.purchase_revision
        LEFT JOIN relay_hosted.latest_text_source_markup m ON m.source_id=i.source_id
        LEFT JOIN relay_hosted.latest_text_sale_review q ON q.scope_id=i.scope_id AND q.model=i.model
        CROSS JOIN LATERAL (SELECT coalesce(q.rates,r.draft) AS rates) draft);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.change_text_rule(origin text,tag bytea,item_id uuid,op uuid,rev bigint,act text,data jsonb)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor_id uuid; proof_at timestamptz; iid uuid; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; review relay_hosted.text_sale_reviews;
    new_id uuid; n bigint; mode text; sale jsonb; cost jsonb; draft jsonb; inputs jsonb; intent jsonb;
    markup jsonb; pct numeric; default_pct numeric; default_rev bigint; source uuid;
    ready boolean; priced boolean;
BEGIN
    SELECT s.installation_id INTO iid FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
        WHERE h.public_origin=origin FOR SHARE OF s;
    SELECT a.user_id,ss.created_at INTO actor_id,proof_at FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions ss ON ss.user_id=a.user_id AND ss.credential_id=a.credential_id
        WHERE ss.verifier=tag AND a.public_origin=origin AND ss.installation_epoch=a.installation_epoch AND ss.user_epoch=a.user_epoch
        AND ss.expires_at>clock_timestamp() AND ss.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF ss,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT * INTO i FROM relay_hosted.text_inventory WHERE id=item_id;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-rule-operation:'||op::text,0));
    PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||i.scope_id::text||':'||i.model,0));
    SELECT * INTO STRICT r FROM relay_hosted.text_model_rules WHERE id=item_id FOR UPDATE;
    intent:=CASE WHEN act='review' THEN data->'rule' ELSE data END;
    SELECT * INTO prior FROM relay_hosted.text_rule_changes WHERE operation=op;
    IF FOUND THEN RETURN CASE WHEN (prior.item,prior.expected,prior.actor,prior.action,prior.payload)=(item_id,rev,actor_id,act,intent)
        THEN 'applied' ELSE 'conflict' END; END IF;
    IF r.revision<>rev THEN RETURN 'conflict'; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.service_control s WHERE s.installation_id=iid AND s.mode='qualification'
        AND s.qualification_scope_ref=i.scope_id) THEN RETURN 'inactive'; END IF;
    IF act NOT IN ('save','review','admit','publish','suspend')
        OR (act NOT IN ('save','review') AND data<>'{}'::jsonb) THEN RETURN 'invalid'; END IF;
    IF act NOT IN ('save','review') AND proof_at<=clock_timestamp()-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    SELECT * INTO review FROM relay_hosted.latest_text_sale_review q WHERE q.scope_id=i.scope_id AND q.model=i.model;

    IF act='save' THEN
        IF NOT r.admitted OR jsonb_typeof(data)<>'object' THEN RETURN 'invalid'; END IF;
        IF relay_hosted.valid_text_rates(data) THEN
            sale:=data;
        ELSIF data<>'{}'::jsonb AND (data-ARRAY['sale','purchase','source_markup'])='{}'::jsonb THEN
            sale:=data->'sale'; cost:=data->'purchase'; markup:=data->'source_markup';
            IF (sale IS NOT NULL AND NOT relay_hosted.valid_text_rates(sale))
                OR (cost IS NOT NULL AND NOT relay_hosted.valid_text_rates(cost)) THEN RETURN 'invalid'; END IF;
        ELSE RETURN 'invalid';
        END IF;
        IF markup IS NOT NULL THEN
            IF jsonb_typeof(markup)<>'object' OR NOT (markup ?& ARRAY['expected_revision','percent'])
                OR (markup-ARRAY['expected_revision','percent'])<>'{}'::jsonb
                OR jsonb_typeof(markup->'expected_revision')<>'string'
                OR markup->>'expected_revision' !~ '^(0|[1-9][0-9]{0,18})$'
                OR (markup->>'expected_revision')::numeric>=9223372036854775807
                OR (markup->'percent'<>'null'::jsonb AND (jsonb_typeof(markup->'percent')<>'string'
                    OR markup->>'percent' !~ '^(0|[1-9][0-9]{0,11})\.[0-9]{12}$')) THEN RETURN 'invalid'; END IF;
            PERFORM pg_advisory_xact_lock_shared(98653249,1);
            IF NOT EXISTS(SELECT FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=iid
                AND s.connection_id=i.source_id AND s.purpose='provider' AND s.generation IS NOT NULL) THEN RETURN 'inactive'; END IF;
            PERFORM pg_advisory_xact_lock(hashtextextended('text-markup:'||i.source_id::text,0));
            SELECT revision INTO default_rev FROM relay_hosted.latest_text_source_markup WHERE source_id=i.source_id;
            default_rev:=coalesce(default_rev,0);
            IF default_rev<>(markup->>'expected_revision')::bigint THEN RETURN 'conflict'; END IF;
            default_pct:=(markup->>'percent')::numeric;
        END IF;
        IF cost IS NOT NULL THEN
            n:=coalesce(r.purchase_revision,0)+1;
            INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
                VALUES(i.id,n,(cost->>'input')::numeric,(cost->>'cached')::numeric,(cost->>'output')::numeric);
            UPDATE relay_hosted.text_model_rules SET purchase_revision=n WHERE id=item_id;
        END IF;
        mode:='fixed';
    ELSIF act='suspend' THEN
        IF NOT r.enabled THEN RETURN 'conflict'; END IF;
        UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=item_id;
    ELSE
        PERFORM pg_advisory_xact_lock_shared(98653249,1);
        IF act='admit' THEN
            IF r.admitted THEN RETURN 'conflict'; END IF;
            IF NOT EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item_id)
                OR relay_hosted.text_inventory_fenced(i.id) THEN RETURN 'inactive'; END IF;
        ELSIF NOT r.admitted THEN RETURN 'invalid'; END IF;
        IF act='review' OR (act='publish' AND review.inputs IS NOT NULL) THEN
            mode:=CASE WHEN act='review' THEN intent->>'mode' ELSE review.mode END;
            IF mode='api_default' THEN
                -- Defaults may be edited through another model of the same source.
                FOR source IN SELECT DISTINCT t.source_id FROM relay_hosted.text_inventory t
                    WHERE t.scope_id=i.scope_id AND t.model=i.model ORDER BY t.source_id
                LOOP
                    PERFORM pg_advisory_xact_lock(hashtextextended('text-markup:'||source::text,0));
                END LOOP;
            END IF;
            inputs:=relay_hosted.text_price_inputs(i,mode='api_default');
        END IF;
        IF act='review' THEN
            IF mode='model_markup' AND (intent-ARRAY['mode','percent'])='{}'::jsonb AND jsonb_typeof(intent->'percent')='string'
                AND intent->>'percent' ~ '^(0|[1-9][0-9]{0,11})\.[0-9]{12}$' THEN
                pct:=(intent->>'percent')::numeric;
            ELSIF intent IS DISTINCT FROM '{"mode":"api_default"}'::jsonb THEN RETURN 'invalid'; END IF;
            IF inputs IS DISTINCT FROM data->'inputs' THEN RETURN 'review_stale'; END IF;
            IF data->>'problem' IN ('no_proposal','unrepresentable') THEN RETURN data->>'problem'; END IF;
            sale:=data->'rates';
            IF relay_hosted.valid_text_rates(sale) IS DISTINCT FROM true THEN RETURN 'invalid'; END IF;
        ELSIF act='admit' THEN
            UPDATE relay_hosted.text_model_rules SET admitted=true WHERE id=item_id;
        ELSE
            draft:=coalesce(review.rates,r.draft);
            IF draft IS NULL THEN RETURN 'invalid'; END IF;
            SELECT count(*)>0,count(v.rate_revision)>0 INTO ready,priced FROM relay_hosted.text_basis_supply(i,true) v;
            IF NOT ready THEN RETURN 'inactive'; END IF;
            IF NOT priced THEN RETURN 'purchase_required'; END IF;
            IF review.inputs IS NOT NULL AND review.inputs IS DISTINCT FROM inputs THEN RETURN 'review_stale'; END IF;
            IF r.enabled AND draft=(SELECT relay_hosted.text_rates(t) FROM relay_hosted.text_offers t WHERE t.id=r.last_offer)
                THEN RETURN 'conflict'; END IF;
            IF EXISTS(SELECT FROM relay_hosted.text_model_rules x WHERE x.scope_id=i.scope_id AND x.model=i.model AND x.id<>i.id AND x.enabled)
                THEN RETURN 'conflict'; END IF;
            SELECT * INTO old FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model AND t.inventory_id IS NOT NULL
                ORDER BY t.revision DESC LIMIT 1;
            IF FOUND AND ((draft->>'input')::numeric>old.input_rate OR (draft->>'cached')::numeric>old.cached_rate
                OR (draft->>'output')::numeric>old.output_rate) THEN RETURN 'notice_required'; END IF;
            n:=coalesce(old.revision,(SELECT max(t.revision) FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model),0)+1;
            new_id:=gen_random_uuid();
            INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
                max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s,sale_review_revision)
                VALUES(new_id,i.id,i.scope_id,i.source_id,i.provider_revision,i.base_url,i.model,n,i.max_input_bytes,i.max_output_tokens,
                    i.tokens_per_byte,i.overhead_tokens,(draft->>'input')::numeric,(draft->>'cached')::numeric,
                    (draft->>'output')::numeric,i.close_after_s,review.revision);
            UPDATE relay_hosted.text_model_rules SET enabled=true,last_offer=new_id,
                first_live_revision=CASE WHEN r.enabled THEN r.first_live_revision ELSE n END WHERE id=item_id;
        END IF;
    END IF;
    INSERT INTO relay_hosted.text_rule_changes(operation,item,actor,expected,action,payload) VALUES(op,item_id,actor_id,rev,act,intent);
    IF default_rev IS NOT NULL THEN
        INSERT INTO relay_hosted.text_source_markups(source_id,revision,percent,operation) VALUES(i.source_id,default_rev+1,default_pct,op);
    END IF;
    IF sale IS NOT NULL THEN
        INSERT INTO relay_hosted.text_sale_reviews(scope_id,model,revision,mode,percent,rates,inputs,operation)
            VALUES(i.scope_id,i.model,coalesce(review.revision,0)+1,mode,pct,sale,inputs,op);
    END IF;
    UPDATE relay_hosted.text_model_rules x SET revision=x.revision+1
        WHERE x.id=item_id OR (sale IS NOT NULL AND x.scope_id=i.scope_id AND x.model=i.model);
    RETURN 'applied';
END $$;

DROP FUNCTION relay_hosted.text_price_inputs(uuid,text,boolean),relay_hosted.text_below_cost(uuid,text,jsonb);
REVOKE ALL ON FUNCTION relay_hosted.text_inventory_fenced(uuid),relay_hosted.text_offer_supply(relay_hosted.text_offers),
    relay_hosted.text_compatible_supply(uuid,text,integer,integer,integer,integer,integer,boolean),
    relay_hosted.text_basis_supply(relay_hosted.text_inventory,boolean),relay_hosted.text_price_inputs(relay_hosted.text_inventory,boolean),
    relay_hosted.text_cost_bound(integer,integer,numeric,numeric,numeric),relay_hosted.text_source_in_flight(uuid),
    relay_hosted.text_source_state(uuid,text,bigint,timestamptz),relay_hosted.request_text_candidates(text,uuid,uuid,uuid,bytea),
    relay_hosted.claim_text_candidate(text,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer,uuid,bigint),
    relay_hosted.record_text_cooldown(uuid,uuid,uuid,bigint),relay_hosted.fence_text_route(uuid,uuid) FROM PUBLIC;
