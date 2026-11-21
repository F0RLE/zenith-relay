-- The lifecycle revision guards owner commands; the inventory epoch retires
-- physical bindings and prepared checks without rewriting publication or holds.
ALTER TABLE relay_hosted.source_serving_changes
    ADD COLUMN action text CHECK (action IN ('pause','resume','park','trash','restore')),
    ADD COLUMN inventory_epoch bigint NOT NULL DEFAULT 0 CHECK (inventory_epoch BETWEEN 0 AND revision),
    ADD CHECK (action IS NULL OR paused=(action<>'resume'));
ALTER TABLE relay_hosted.text_inventory ADD COLUMN inventory_epoch bigint NOT NULL DEFAULT 0 CHECK (inventory_epoch>=0);
ALTER TABLE relay_hosted.text_check_attempts ADD COLUMN inventory_epoch bigint NOT NULL DEFAULT 0 CHECK (inventory_epoch>=0);

CREATE OR REPLACE VIEW relay_hosted.source_serving AS
    SELECT DISTINCT ON (installation_id,source_id) installation_id,source_id,revision,paused,
        coalesce(action,CASE WHEN paused THEN 'pause' ELSE 'resume' END) AS action,inventory_epoch
    FROM relay_hosted.source_serving_changes ORDER BY installation_id,source_id,revision DESC;

CREATE OR REPLACE VIEW relay_hosted.current_text_inventory AS
    SELECT i.id,i.scope_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy,i.base_url,i.model,i.source_label,
        i.max_input_bytes,i.max_output_tokens,i.tokens_per_byte,i.overhead_tokens,i.close_after_s,i.checked_at,i.valid_until,
        s.installation_id,h.public_origin
    FROM relay_hosted.text_inventory i
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id AND s.mode='qualification'
    JOIN relay_hosted.installations h ON h.id=s.installation_id
    JOIN relay_hosted.configuration_revisions q ON q.id=i.scope_id
    LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(s.installation_id,i.source_id)
    WHERE i.inventory_epoch=coalesce(v.inventory_epoch,0) AND least(q.expires_at,i.valid_until)>clock_timestamp()
        AND relay_hosted.text_source_matches(h.id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy);

-- Admission to physical supply is scoped to the checked incarnation. A live
-- rule may still anchor the offer after that binding retires, via other supply.
CREATE OR REPLACE VIEW relay_hosted.text_supply_binding AS
    SELECT i.id,i.scope_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy,i.base_url,i.model,i.source_label,
        i.max_input_bytes,i.max_output_tokens,i.tokens_per_byte,i.overhead_tokens,i.close_after_s,i.checked_at,i.valid_until,
        i.direct_ipv4,q.expires_at AS scope_expires_at,p.revision AS rate_revision,p.input_rate,p.cached_rate,p.output_rate
    FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.admitted
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id AND s.mode='qualification'
    JOIN relay_hosted.configuration_revisions q ON q.id=i.scope_id
    LEFT JOIN relay_hosted.text_purchase_rates p ON (p.item,p.revision)=(i.id,r.purchase_revision)
    LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(s.installation_id,i.source_id)
    WHERE i.inventory_epoch=coalesce(v.inventory_epoch,0)
        AND relay_hosted.text_source_matches(s.installation_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy);

CREATE FUNCTION relay_hosted.source_in_pool(scope uuid,source uuid,epoch bigint)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r USING(id)
        WHERE (i.scope_id,i.source_id,i.inventory_epoch)=($1,$2,$3) AND r.admitted)
$$;

CREATE FUNCTION relay_hosted.source_serving_view(iid uuid,source uuid)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('revision',coalesce(v.revision,0)::text,
        'state',CASE WHEN v.action='trash' THEN 'trashed' WHEN v.paused THEN 'paused' ELSE 'enabled' END,
        'in_pool',coalesce(relay_hosted.source_in_pool(s.qualification_scope_ref,c.connection_id,coalesce(v.inventory_epoch,0)),false),
        'draining',coalesce(v.paused,false) AND
            (relay_hosted.text_source_in_flight(c.connection_id) OR relay_hosted.text_probe_in_flight(c.connection_id)))
    FROM relay_hosted.connection_secret_slots c JOIN relay_hosted.service_control s USING(installation_id)
    LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(c.installation_id,c.connection_id)
    WHERE (c.installation_id,c.connection_id,c.purpose)=($1,$2,'provider') AND c.generation IS NOT NULL
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; page jsonb;
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    page:=relay_hosted.connection_page(iid,after_id);
    -- Keep the paired Desktop wire schema unchanged.
    RETURN jsonb_set(page,'{items}',(SELECT coalesce(jsonb_agg(item||jsonb_build_object(
        'serving',relay_hosted.source_serving_view(iid,(item->>'id')::uuid)) ORDER BY n),'[]'::jsonb)
        FROM jsonb_array_elements(page->'items') WITH ORDINALITY AS rows(item,n)));
END $$;

DROP FUNCTION relay_hosted.change_source_serving(text,bytea,uuid,uuid,bigint,boolean,bytea,bigint,bigint,boolean,boolean,bigint);
CREATE FUNCTION relay_hosted.change_source_serving(origin text,tag bytea,source uuid,op uuid,expected bigint,act text,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; scope uuid; prior relay_hosted.source_serving_changes; v relay_hosted.source_serving;
    epoch bigint; model_id text; proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF source IS NULL OR source='00000000-0000-0000-0000-000000000000'
        OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 0 AND 9223372036854775806
        OR act IS NULL OR act NOT IN ('pause','resume','park','trash','restore') THEN RETURN 'invalid'; END IF;
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

    -- The session's service row lock excludes new qualification evidence.
    -- Serialize with admission and price review for the affected models only.
    FOR model_id IN SELECT DISTINCT i.model FROM relay_hosted.text_inventory i
        WHERE (i.scope_id,i.source_id)=(scope,source) ORDER BY i.model
    LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||scope::text||':'||model_id,0));
    END LOOP;
    -- Claim can hold model-rule row locks while waiting here; do not write them.
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    PERFORM FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=iid AND s.connection_id=source
        AND s.purpose='provider' AND s.generation IS NOT NULL FOR SHARE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO v FROM relay_hosted.source_serving s WHERE (s.installation_id,s.source_id)=(iid,source);
    epoch:=coalesce(v.inventory_epoch,0);
    IF coalesce(v.revision,0)<>expected OR (v.action IS NOT DISTINCT FROM 'trash')<>(act='restore')
        OR (act='pause' AND v.paused) OR (act='resume' AND NOT coalesce(v.paused,false))
        OR (act='park' AND NOT coalesce(relay_hosted.source_in_pool(scope,source,epoch),false)) THEN RETURN 'conflict'; END IF;
    IF act='resume' AND (relay_hosted.text_source_in_flight(source) OR relay_hosted.text_probe_in_flight(source))
        THEN RETURN 'draining'; END IF;
    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.source_serving_changes(installation_id,source_id,revision,operation_id,actor,paused,changed_at,
        confirmed_by_passkey_id,action,inventory_epoch)
        VALUES(iid,source,expected+1,op,ss.user_id,act<>'resume',stamp,confirmed,act,
            CASE WHEN act IN ('park','trash','restore') THEN expected+1 ELSE epoch END);
    RETURN 'applied';
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.text_check_step(origin text,scope uuid,item uuid,source uuid,provider_rev bigint,proxy_rev bigint,
    proxied boolean,profile jsonb,step text)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; until_at timestamptz; job relay_hosted.text_check_attempts; profile_digest bytea; v relay_hosted.source_serving;
BEGIN
    IF jsonb_typeof(profile)<>'object' OR octet_length(profile::text)>8192 OR step NOT IN ('prepare','claim','release')
        OR NOT profile @> jsonb_build_object('id',item,'scope',scope,'source',source,'provider_revision',provider_rev,
            'proxy_revision',proxy_rev,'uses_proxy',proxied) THEN RETURN 'denied'; END IF;
    profile_digest:=sha256(convert_to(profile::text,'UTF8'));
    SELECT h.id,q.expires_at INTO iid,until_at FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.id=scope FOR UPDATE OF s;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    IF iid IS NULL OR until_at<=clock_timestamp() OR NOT relay_hosted.text_source_matches(iid,source,provider_rev,proxy_rev,proxied)
        THEN RETURN 'denied'; END IF;
    SELECT * INTO v FROM relay_hosted.source_serving s WHERE (s.installation_id,s.source_id)=(iid,source);
    SELECT * INTO job FROM relay_hosted.text_check_attempts WHERE id=item FOR UPDATE;
    IF FOUND THEN
        IF job.scope_id<>scope OR job.source_id<>source OR job.digest<>profile_digest THEN RETURN 'conflict'; END IF;
        IF step='release' AND job.claimed THEN
            -- The qualifier can abandon its own claim if no generation was sent.
            UPDATE relay_hosted.text_check_attempts SET expires_at=least(expires_at,clock_timestamp()) WHERE id=item;
            RETURN 'released';
        END IF;
        IF job.inventory_epoch<>coalesce(v.inventory_epoch,0) THEN RETURN 'denied'; END IF;
        IF EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item) THEN RETURN 'done'; END IF;
        IF v.paused OR step='release' THEN RETURN 'denied'; END IF;
        IF job.claimed OR job.expires_at<=clock_timestamp() THEN RETURN 'unconfirmed'; END IF;
        IF step='claim' THEN
            IF NOT pg_try_advisory_xact_lock(hashtextextended('text-source:'||source::text,0)) OR relay_hosted.text_probe_in_flight(source)
                OR relay_hosted.text_source_in_flight(source) THEN RETURN 'busy'; END IF;
            UPDATE relay_hosted.text_check_attempts SET claimed=true,expires_at=clock_timestamp()+interval '2 minutes' WHERE id=item;
            RETURN 'claimed';
        END IF;
        RETURN 'prepared';
    END IF;
    IF v.paused OR step<>'prepare' OR (SELECT count(*) FROM relay_hosted.text_check_attempts WHERE scope_id=scope)>=128
        THEN RETURN 'denied'; END IF;
    INSERT INTO relay_hosted.text_check_attempts(id,scope_id,source_id,digest,expires_at,inventory_epoch)
        VALUES(item,scope,source,profile_digest,least(until_at,clock_timestamp()+interval '2 minutes'),coalesce(v.inventory_epoch,0));
    RETURN 'prepared';
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.record_text_check(origin text,scope uuid,item uuid,source uuid,provider_rev bigint,
    proxy_rev bigint,proxied boolean,endpoint text,model_id text,label text,input_limit integer,output_limit integer,
    multiplier integer,overhead integer,closure integer,lifetime integer,pin inet)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; expiry timestamptz; stamp timestamptz; prev relay_hosted.text_inventory; expected bytea; profile jsonb; epoch bigint;
BEGIN
    IF pin IS NULL THEN
        IF endpoint NOT LIKE 'http://%' THEN RETURN false; END IF;
    ELSIF family(pin)<>4 OR masklen(pin)<>32 OR proxied OR endpoint NOT LIKE 'https://%' THEN RETURN false;
    END IF;
    profile:=jsonb_build_object('id',item,'scope',scope,'source',source,'source_label',label,'endpoint',endpoint,
        'model',model_id,'provider_revision',provider_rev,'proxy_revision',proxy_rev,'uses_proxy',proxied,
        'max_input_bytes',input_limit,'max_output_tokens',output_limit,'tokens_per_byte',multiplier,'overhead_tokens',overhead,
        'close_after_s',closure,'valid_for_s',lifetime);
    IF pin IS NOT NULL THEN profile:=profile||jsonb_build_object('direct_ipv4',host(pin)); END IF;
    expected:=sha256(convert_to(profile::text,'UTF8'));
    SELECT h.id,q.expires_at INTO iid,expiry FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.id=scope FOR UPDATE OF s;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT v.inventory_epoch INTO epoch FROM relay_hosted.source_serving v WHERE (v.installation_id,v.source_id)=(iid,source);
    epoch:=coalesce(epoch,0);
    stamp:=clock_timestamp();
    IF iid IS NULL OR expiry<=stamp OR lifetime NOT BETWEEN 60 AND 86400 OR NOT EXISTS(
        SELECT FROM relay_hosted.text_check_attempts j WHERE j.id=item AND j.scope_id=scope AND j.source_id=source
            AND j.digest=expected AND j.claimed AND j.expires_at>stamp AND j.inventory_epoch=epoch)
        OR NOT relay_hosted.text_source_matches(iid,source,provider_rev,proxy_rev,proxied) THEN RETURN false; END IF;
    SELECT * INTO prev FROM relay_hosted.text_inventory WHERE id=item;
    IF FOUND THEN RETURN (prev.scope_id,prev.source_id,prev.provider_revision,prev.proxy_revision,prev.uses_proxy,prev.base_url,
        prev.model,prev.source_label,prev.max_input_bytes,prev.max_output_tokens,prev.tokens_per_byte,prev.overhead_tokens,
        prev.close_after_s,prev.inventory_epoch)
        =(scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,input_limit,output_limit,multiplier,overhead,closure,epoch)
        AND prev.direct_ipv4 IS NOT DISTINCT FROM pin AND prev.valid_until>stamp; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_inventory WHERE scope_id=scope)>=128 THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,base_url,
        model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,checked_at,valid_until,
        direct_ipv4,inventory_epoch)
        VALUES(item,scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,input_limit,output_limit,multiplier,overhead,
            closure,stamp,least(expiry,stamp+lifetime*interval '1 second'),pin,epoch);
    INSERT INTO relay_hosted.text_model_rules(id,scope_id,model) VALUES(item,scope,model_id);
    RETURN true;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,'source',i.source_label,'proxy',i.uses_proxy,
        'serving',relay_hosted.source_serving_view(h.id,i.source_id),
        'revision',r.revision::text,'admitted',r.admitted AND current_binding.active,'enabled',r.enabled,
        'ready',c.id IS NOT NULL AND NOT relay_hosted.text_inventory_fenced(i.id),
        'supply_ready',EXISTS(SELECT FROM relay_hosted.text_basis_supply(i,true) p WHERE p.rate_revision IS NOT NULL),
        'purchase',CASE WHEN p.item IS NOT NULL AND current_binding.active THEN
            jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,'output',p.output_rate::text) END,
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
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(h.id,i.source_id)
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id
        LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=r.purchase_revision
        LEFT JOIN relay_hosted.latest_text_source_markup m ON m.source_id=i.source_id
        LEFT JOIN relay_hosted.latest_text_sale_review q ON q.scope_id=i.scope_id AND q.model=i.model
        CROSS JOIN LATERAL (SELECT coalesce(q.rates,r.draft) AS rates) draft
        CROSS JOIN LATERAL (SELECT i.inventory_epoch=coalesce(v.inventory_epoch,0) AS active) current_binding
        WHERE current_binding.active OR r.enabled);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.change_text_rule(origin text,tag bytea,item_id uuid,op uuid,rev bigint,act text,data jsonb,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; review relay_hosted.text_sale_reviews;
    new_id uuid; n bigint; mode text; sale jsonb; cost jsonb; draft jsonb; inputs jsonb; intent jsonb;
    markup jsonb; pct numeric; default_pct numeric; default_rev bigint; source uuid; lifecycle relay_hosted.source_serving;
    ready boolean; priced boolean; proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF item_id IS NULL OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR rev IS NULL OR rev NOT BETWEEN 0 AND 9223372036854775806
        OR act IS NULL OR act NOT IN ('save','review','admit','publish','suspend') OR data IS NULL
        OR (act NOT IN ('save','review') AND data<>'{}'::jsonb)
        OR (act IN ('save','review') AND proof_credential IS NOT NULL) THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-rule-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN 'denied'; END IF;
    intent:=CASE WHEN act='review' THEN data->'rule' ELSE data END;
    SELECT * INTO prior FROM relay_hosted.text_rule_changes WHERE operation=op;
    IF FOUND THEN RETURN CASE WHEN (prior.item,prior.expected,prior.actor,prior.action,prior.payload)
        =(item_id,rev,ss.user_id,act,intent) THEN 'applied' ELSE 'conflict' END; END IF;

    SELECT * INTO i FROM relay_hosted.text_inventory WHERE id=item_id;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.service_control s WHERE s.installation_id=iid AND s.mode='qualification'
        AND s.qualification_scope_ref=i.scope_id) THEN RETURN 'inactive'; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true
        THEN RETURN 'passkey_changed'; END IF;
    IF act IN ('admit','review','publish') OR (act='save' AND data ? 'source_markup') THEN
        PERFORM pg_advisory_xact_lock_shared(98653249,1);
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||i.scope_id::text||':'||i.model,0));
    IF act='review' OR (act='save' AND data ?| ARRAY['input','sale']) THEN
        -- Keep the dispatch lock order: live rule, then supplying rule.
        PERFORM FROM relay_hosted.text_model_rules x
            WHERE (x.scope_id,x.model)=(i.scope_id,i.model) AND x.enabled FOR NO KEY UPDATE;
    END IF;
    SELECT * INTO STRICT r FROM relay_hosted.text_model_rules WHERE id=item_id FOR NO KEY UPDATE;
    IF r.revision<>rev THEN RETURN 'conflict'; END IF;
    SELECT * INTO lifecycle FROM relay_hosted.source_serving s WHERE (s.installation_id,s.source_id)=(iid,i.source_id);
    -- An old anchor still owns its published offer, but cannot be reused as a
    -- purchase-rate editor or as permission to readmit the retired binding.
    IF (act='admit' AND lifecycle.paused) OR (i.inventory_epoch<>coalesce(lifecycle.inventory_epoch,0)
        AND (act='admit' OR NOT r.enabled OR (act='save' AND data ?| ARRAY['purchase','source_markup'])))
        THEN RETURN 'inactive'; END IF;
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
            IF NOT EXISTS(SELECT FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=iid
                AND s.connection_id=i.source_id AND s.purpose='provider' AND s.generation IS NOT NULL) THEN RETURN 'inactive'; END IF;
            PERFORM pg_advisory_xact_lock(hashtextextended('text-markup:'||i.source_id::text,0));
            SELECT revision INTO default_rev FROM relay_hosted.latest_text_source_markup WHERE source_id=i.source_id;
            default_rev:=coalesce(default_rev,0);
            IF default_rev<>(markup->>'expected_revision')::bigint THEN RETURN 'conflict'; END IF;
            default_pct:=(markup->>'percent')::numeric;
        END IF;
        mode:='fixed';
    ELSIF act='suspend' THEN
        IF NOT r.enabled THEN RETURN 'conflict'; END IF;
    ELSIF act='admit' THEN
        IF r.admitted THEN RETURN 'conflict'; END IF;
        IF NOT EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item_id)
            OR relay_hosted.text_inventory_fenced(i.id) THEN RETURN 'inactive'; END IF;
    ELSE
        IF NOT r.admitted THEN RETURN 'invalid'; END IF;
        IF act='review' OR review.inputs IS NOT NULL THEN
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
        ELSE
            draft:=coalesce(review.rates,r.draft);
            IF draft IS NULL THEN RETURN 'invalid'; END IF;
            stamp:=clock_timestamp();
            SELECT count(*)>0,count(v.rate_revision)>0 INTO ready,priced FROM relay_hosted.text_basis_supply(i,true) v
                WHERE least(v.valid_until,v.scope_expires_at)>stamp;
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
        END IF;
    END IF;

    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    IF act IN ('admit','publish','suspend') THEN
        IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
        SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
            ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
        IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    END IF;

    IF cost IS NOT NULL THEN
        n:=coalesce(r.purchase_revision,0)+1;
        INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
            VALUES(i.id,n,(cost->>'input')::numeric,(cost->>'cached')::numeric,(cost->>'output')::numeric);
        UPDATE relay_hosted.text_model_rules SET purchase_revision=n WHERE id=item_id;
    END IF;
    IF act='suspend' THEN
        UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=item_id;
    ELSIF act='admit' THEN
        UPDATE relay_hosted.text_model_rules SET admitted=true WHERE id=item_id;
    ELSIF act='publish' THEN
        new_id:=gen_random_uuid();
        INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
            max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s,sale_review_revision)
            VALUES(new_id,i.id,i.scope_id,i.source_id,i.provider_revision,i.base_url,i.model,n,i.max_input_bytes,i.max_output_tokens,
                i.tokens_per_byte,i.overhead_tokens,(draft->>'input')::numeric,(draft->>'cached')::numeric,
                (draft->>'output')::numeric,i.close_after_s,review.revision);
        UPDATE relay_hosted.text_model_rules SET enabled=true,last_offer=new_id,
            first_live_revision=CASE WHEN r.enabled THEN r.first_live_revision ELSE n END WHERE id=item_id;
    END IF;
    INSERT INTO relay_hosted.text_rule_changes(operation,item,actor,expected,action,payload,at,confirmed_by_passkey_id)
        VALUES(op,item_id,ss.user_id,rev,act,intent,stamp,confirmed);
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

REVOKE ALL ON FUNCTION relay_hosted.source_in_pool(uuid,uuid,bigint),relay_hosted.source_serving_view(uuid,uuid),
    relay_hosted.change_source_serving(text,bytea,uuid,uuid,bigint,text,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
