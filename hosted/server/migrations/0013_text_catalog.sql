-- A checked source is parked inventory. Neither an observation nor a saved draft grants access.
CREATE TABLE relay_hosted.text_inventory (
    id uuid PRIMARY KEY, scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    source_id uuid NOT NULL, provider_revision bigint NOT NULL CHECK (provider_revision>0),
    proxy_revision bigint NOT NULL CHECK (proxy_revision>=0), uses_proxy boolean NOT NULL,
    base_url text NOT NULL CHECK (octet_length(base_url) BETWEEN 1 AND 256),
    model text COLLATE "C" NOT NULL CHECK (model ~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$'),
    source_label text NOT NULL CHECK (octet_length(source_label) BETWEEN 1 AND 96 AND source_label !~ '[[:cntrl:]]'),
    max_input_bytes integer NOT NULL CHECK (max_input_bytes BETWEEN 1 AND 60000),
    max_output_tokens integer NOT NULL CHECK (max_output_tokens BETWEEN 1 AND 1000000),
    tokens_per_byte integer NOT NULL CHECK (tokens_per_byte BETWEEN 1 AND 16),
    overhead_tokens integer NOT NULL CHECK (overhead_tokens BETWEEN 0 AND 65536),
    close_after_s integer NOT NULL CHECK (close_after_s BETWEEN 65 AND 3600),
    checked_at timestamptz NOT NULL DEFAULT clock_timestamp(), valid_until timestamptz NOT NULL,
    CHECK (valid_until>checked_at AND valid_until<=checked_at+interval '24 hours'),
    CHECK (NOT uses_proxy OR proxy_revision>0), UNIQUE(id,scope_id,model)
);
CREATE TRIGGER sealed_text_observation BEFORE UPDATE OR DELETE ON relay_hosted.text_inventory
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
ALTER TABLE relay_hosted.text_offers ADD COLUMN inventory_id uuid,
    ADD FOREIGN KEY(inventory_id,scope_id,model) REFERENCES relay_hosted.text_inventory(id,scope_id,model);

CREATE TABLE relay_hosted.text_model_rules (
    id uuid PRIMARY KEY, scope_id uuid NOT NULL, model text COLLATE "C" NOT NULL,
    FOREIGN KEY(id,scope_id,model) REFERENCES relay_hosted.text_inventory(id,scope_id,model),
    revision bigint NOT NULL DEFAULT 0 CHECK (revision>=0), admitted boolean NOT NULL DEFAULT false,
    draft jsonb, last_offer uuid REFERENCES relay_hosted.text_offers, enabled boolean NOT NULL DEFAULT false,
    first_live_revision bigint CHECK (first_live_revision>0),
    CHECK (NOT enabled OR (admitted AND last_offer IS NOT NULL AND first_live_revision IS NOT NULL))
);
CREATE UNIQUE INDEX one_published_text_model ON relay_hosted.text_model_rules(scope_id,model) WHERE enabled;
CREATE TABLE relay_hosted.text_rule_changes (
    operation uuid PRIMARY KEY, item uuid NOT NULL REFERENCES relay_hosted.text_model_rules,
    actor uuid NOT NULL REFERENCES relay_hosted.users, expected bigint NOT NULL,
    action text NOT NULL CHECK (action IN ('admit','save','publish','suspend')),
    payload jsonb NOT NULL, at timestamptz NOT NULL DEFAULT clock_timestamp(), UNIQUE(item,expected)
);
CREATE TRIGGER immutable_rule_change BEFORE UPDATE OR DELETE ON relay_hosted.text_rule_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.text_source_matches(iid uuid,source uuid,provider_rev bigint,proxy_rev bigint,proxied boolean)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.connection_secret_slots v WHERE v.installation_id=iid
        AND v.connection_id=source AND v.purpose='provider' AND v.revision=provider_rev AND v.generation IS NOT NULL)
    AND coalesce((SELECT p.revision=proxy_rev AND (p.generation IS NOT NULL)=proxied FROM relay_hosted.connection_secret_slots p
        WHERE p.installation_id=iid AND p.connection_id=source AND p.purpose='proxy'),proxy_rev=0 AND NOT proxied)
$$;
CREATE VIEW relay_hosted.current_text_inventory AS
    SELECT i.*,s.installation_id,h.public_origin FROM relay_hosted.text_inventory i
    JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id AND s.mode='qualification'
    JOIN relay_hosted.installations h ON h.id=s.installation_id JOIN relay_hosted.configuration_revisions q ON q.id=i.scope_id
    WHERE least(q.expires_at,i.valid_until)>clock_timestamp()
        AND relay_hosted.text_source_matches(h.id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy);
CREATE VIEW relay_hosted.active_text_listings AS
    SELECT i.public_origin,i.id AS item,o.* FROM relay_hosted.current_text_inventory i
    JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.enabled AND r.admitted
    JOIN relay_hosted.text_offers o ON o.id=r.last_offer AND o.inventory_id=i.id
    WHERE NOT EXISTS(SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id
        WHERE t.inventory_id=i.id);

-- The private qualifier alone can record evidence; it cannot admit or publish a model.
CREATE TABLE relay_hosted.text_check_attempts (
    id uuid PRIMARY KEY, scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    source_id uuid NOT NULL, digest bytea NOT NULL CHECK(octet_length(digest)=32), expires_at timestamptz NOT NULL, claimed boolean NOT NULL DEFAULT false
);
CREATE INDEX pending_text_source_checks ON relay_hosted.text_check_attempts(source_id,expires_at) WHERE claimed;
CREATE FUNCTION relay_hosted.text_probe_in_flight(source uuid)
RETURNS boolean LANGUAGE sql VOLATILE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.text_check_attempts j WHERE j.source_id=source AND j.claimed AND j.expires_at>clock_timestamp()
        AND NOT EXISTS(SELECT FROM relay_hosted.text_inventory i WHERE i.id=j.id))
$$;
CREATE FUNCTION relay_hosted.text_check_scope(origin text)
RETURNS uuid LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT q.id FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.expires_at>statement_timestamp()
$$;
CREATE FUNCTION relay_hosted.text_check_step(origin text,scope uuid,item uuid,source uuid,provider_rev bigint,proxy_rev bigint,
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
                OR EXISTS(SELECT FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
                    WHERE o.source_id=source AND r.closed_at IS NULL) THEN RETURN 'busy'; END IF;
            UPDATE relay_hosted.text_check_attempts SET claimed=true,expires_at=clock_timestamp()+interval '2 minutes' WHERE id=item;
            RETURN 'claimed';
        END IF;
        RETURN 'prepared';
    END IF;
    IF step<>'prepare' OR (SELECT count(*) FROM relay_hosted.text_check_attempts WHERE scope_id=scope)>=128 THEN RETURN 'denied'; END IF;
    INSERT INTO relay_hosted.text_check_attempts VALUES(item,scope,source,profile_digest,least(until_at,clock_timestamp()+interval '2 minutes'),false);
    RETURN 'prepared';
END $$;
CREATE FUNCTION relay_hosted.record_text_check(origin text, scope uuid, item uuid, source uuid, provider_rev bigint,
    proxy_rev bigint, proxied boolean, endpoint text, model_id text, label text, input_limit integer, output_limit integer,
    multiplier integer, overhead integer, closure integer, lifetime integer)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; expiry timestamptz; stamp timestamptz; prev relay_hosted.text_inventory; expected bytea;
BEGIN
    expected:=sha256(convert_to(jsonb_build_object('id',item,'scope',scope,'source',source,'source_label',label,'endpoint',endpoint,
        'model',model_id,'provider_revision',provider_rev,'proxy_revision',proxy_rev,'uses_proxy',proxied,
        'max_input_bytes',input_limit,'max_output_tokens',output_limit,'tokens_per_byte',multiplier,'overhead_tokens',overhead,
        'close_after_s',closure,'valid_for_s',lifetime)::text,'UTF8'));
    SELECT h.id,q.expires_at INTO iid,expiry FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.id=scope FOR UPDATE OF s;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    stamp:=clock_timestamp();
    IF iid IS NULL OR expiry<=stamp OR lifetime NOT BETWEEN 60 AND 86400 OR NOT EXISTS(
        SELECT FROM relay_hosted.text_check_attempts j WHERE j.id=item AND j.scope_id=scope AND j.source_id=source
            AND j.digest=expected AND j.claimed AND j.expires_at>stamp)
        OR NOT relay_hosted.text_source_matches(iid,source,provider_rev,proxy_rev,proxied) THEN RETURN false; END IF;
    SELECT * INTO prev FROM relay_hosted.text_inventory WHERE id=item;
    IF FOUND THEN RETURN (prev.scope_id,prev.source_id,prev.provider_revision,prev.proxy_revision,prev.uses_proxy,prev.base_url,
        prev.model,prev.source_label,prev.max_input_bytes,prev.max_output_tokens,prev.tokens_per_byte,prev.overhead_tokens,prev.close_after_s)
        =(scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,input_limit,output_limit,multiplier,overhead,closure)
        AND prev.valid_until>stamp; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_inventory WHERE scope_id=scope)>=128 THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_inventory VALUES(item,scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,
        input_limit,output_limit,multiplier,overhead,closure,stamp,least(expiry,stamp+lifetime*interval '1 second'));
    INSERT INTO relay_hosted.text_model_rules(id,scope_id,model) VALUES(item,scope,model_id);
    RETURN true;
END $$;

CREATE FUNCTION relay_hosted.text_rates(o relay_hosted.text_offers)
RETURNS jsonb LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('input',o.input_rate::text,'cached',o.cached_rate::text,'output',o.output_rate::text)
$$;
CREATE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,'source',i.source_label,'proxy',i.uses_proxy,
        'revision',r.revision::text,'admitted',r.admitted,'enabled',r.enabled,'ready',c.id IS NOT NULL AND NOT EXISTS(
            SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id WHERE t.inventory_id=i.id),
        'draft',r.draft,'effective',CASE WHEN l.id IS NOT NULL THEN relay_hosted.text_rates(o) END)
        ORDER BY i.model,i.source_label,i.id),'[]') FROM relay_hosted.text_inventory i
        JOIN relay_hosted.text_model_rules r ON r.id=i.id JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id);
END $$;
CREATE FUNCTION relay_hosted.change_text_rule(origin text, tag bytea, item_id uuid, op uuid, rev bigint, act text, data jsonb)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor_id uuid; proof_at timestamptz; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; new_id uuid; n bigint; k text;
BEGIN
    -- Lock control before credentials, like mode transitions. Serialize publications of the same public ID.
    PERFORM FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
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
    SELECT * INTO prior FROM relay_hosted.text_rule_changes WHERE operation=op;
    IF FOUND THEN RETURN CASE WHEN (prior.item,prior.expected,prior.actor,prior.action,prior.payload)=(item_id,rev,actor_id,act,data)
        THEN 'applied' ELSE 'conflict' END; END IF;
    IF r.revision<>rev THEN RETURN 'conflict'; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.service_control WHERE mode='qualification' AND qualification_scope_ref=i.scope_id)
        THEN RETURN 'inactive'; END IF;
    IF act NOT IN ('save','admit','publish','suspend') OR (act<>'save' AND data<>'{}'::jsonb) THEN RETURN 'invalid'; END IF;
    IF act<>'save' AND proof_at<=clock_timestamp()-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    IF act='save' THEN
        IF NOT r.admitted OR jsonb_typeof(data)<>'object' OR NOT (data ?& ARRAY['input','cached','output'])
            OR (data-ARRAY['input','cached','output'])<>'{}'::jsonb THEN RETURN 'invalid'; END IF;
        FOREACH k IN ARRAY ARRAY['input','cached','output'] LOOP
            IF jsonb_typeof(data->k)<>'string' OR (data->>k) !~ '^(0|[1-9][0-9]{0,11})\.[0-9]{12}$' THEN RETURN 'invalid'; END IF;
        END LOOP;
        UPDATE relay_hosted.text_model_rules SET draft=data WHERE id=item_id;
    ELSIF act='suspend' THEN
        IF NOT r.enabled THEN RETURN 'conflict'; END IF;
        UPDATE relay_hosted.text_model_rules SET enabled=false WHERE id=item_id;
    ELSE
        PERFORM pg_advisory_xact_lock_shared(98653249,1);
        IF NOT EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item_id)
            OR EXISTS(SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id WHERE t.inventory_id=item_id)
            THEN RETURN 'inactive'; END IF;
        IF act='admit' THEN
            IF r.admitted THEN RETURN 'conflict'; END IF;
            UPDATE relay_hosted.text_model_rules SET admitted=true WHERE id=item_id;
        ELSE
            IF NOT r.admitted OR r.draft IS NULL THEN RETURN 'invalid'; END IF;
            IF r.enabled AND r.draft=(SELECT relay_hosted.text_rates(t) FROM relay_hosted.text_offers t WHERE t.id=r.last_offer)
                THEN RETURN 'conflict'; END IF;
            IF EXISTS(SELECT FROM relay_hosted.text_model_rules x WHERE x.scope_id=i.scope_id AND x.model=i.model AND x.id<>i.id AND x.enabled)
                THEN RETURN 'conflict'; END IF;
            -- Do not let a second observation or a suspension disguise an unannounced price increase.
            SELECT * INTO old FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model AND t.inventory_id IS NOT NULL
                ORDER BY t.revision DESC LIMIT 1;
            IF FOUND AND ((r.draft->>'input')::numeric>old.input_rate OR (r.draft->>'cached')::numeric>old.cached_rate
                OR (r.draft->>'output')::numeric>old.output_rate) THEN RETURN 'notice_required'; END IF;
            SELECT coalesce(max(t.revision),0)+1 INTO n FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model;
            new_id:=gen_random_uuid();
            INSERT INTO relay_hosted.text_offers(id,inventory_id,scope_id,source_id,provider_revision,base_url,model,revision,
                max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,input_rate,cached_rate,output_rate,close_after_s)
                VALUES(new_id,i.id,i.scope_id,i.source_id,i.provider_revision,i.base_url,i.model,n,i.max_input_bytes,i.max_output_tokens,
                    i.tokens_per_byte,i.overhead_tokens,(r.draft->>'input')::numeric,(r.draft->>'cached')::numeric,
                    (r.draft->>'output')::numeric,i.close_after_s);
            UPDATE relay_hosted.text_model_rules SET enabled=true,last_offer=new_id,
                first_live_revision=CASE WHEN r.enabled THEN r.first_live_revision ELSE n END WHERE id=item_id;
        END IF;
    END IF;
    INSERT INTO relay_hosted.text_rule_changes(operation,item,actor,expected,action,payload) VALUES(op,item_id,actor_id,rev,act,data);
    UPDATE relay_hosted.text_model_rules SET revision=revision+1 WHERE id=item_id;
    RETURN 'applied';
END $$;
CREATE FUNCTION relay_hosted.customer_text_models(origin text,provider uuid,binding bytea,token bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('model',l.model,'protocol','responses','revision',l.revision::text,
        'effective_at_s',floor(extract(epoch FROM l.created_at))::bigint,'rates',relay_hosted.text_rates(o)) ORDER BY l.model),'[]')
        FROM relay_hosted.active_text_listings l JOIN relay_hosted.text_offers o ON o.id=l.id
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id AND q.workspace_id=space WHERE l.public_origin=origin);
END $$;
CREATE FUNCTION relay_hosted.key_text_models(origin text,kid uuid,proof bytea)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH authorized AS (SELECT relay_hosted.qualified_key_workspace(origin,kid,proof) AS space)
    SELECT CASE WHEN space IS NOT NULL THEN (SELECT coalesce(jsonb_agg(l.model ORDER BY l.model),'[]')
        FROM relay_hosted.active_text_listings l JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id
        WHERE q.workspace_id=space AND l.public_origin=origin) END FROM authorized
$$;
REVOKE ALL ON FUNCTION relay_hosted.text_source_matches(uuid,uuid,bigint,bigint,boolean),
    relay_hosted.text_check_scope(text),relay_hosted.text_probe_in_flight(uuid),
    relay_hosted.text_check_step(text,uuid,uuid,uuid,bigint,bigint,boolean,jsonb,text),
    relay_hosted.record_text_check(text,uuid,uuid,uuid,bigint,bigint,boolean,text,text,text,integer,integer,integer,integer,integer,integer),
    relay_hosted.text_rates(relay_hosted.text_offers),relay_hosted.owner_text_rules(text,bytea),
    relay_hosted.change_text_rule(text,bytea,uuid,uuid,bigint,text,jsonb),
    relay_hosted.customer_text_models(text,uuid,bytea,bytea),relay_hosted.key_text_models(text,uuid,bytea) FROM PUBLIC;
