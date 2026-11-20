-- A supplier default belongs to the connection. A reviewed sale belongs to the offer.
CREATE TABLE relay_hosted.text_source_markups (
    source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    percent numeric(30,12) CHECK (percent>=0 AND percent<1e12),
    operation uuid NOT NULL UNIQUE REFERENCES relay_hosted.text_rule_changes,
    PRIMARY KEY(source_id,revision)
);
CREATE TABLE relay_hosted.text_sale_reviews (
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    model text COLLATE "C" NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    mode text NOT NULL CHECK (mode IN ('fixed','api_default','model_markup')),
    percent numeric(30,12) CHECK (percent>=0 AND percent<1e12),
    rates jsonb NOT NULL CHECK (relay_hosted.valid_text_rates(rates)),
    inputs jsonb,
    operation uuid NOT NULL UNIQUE REFERENCES relay_hosted.text_rule_changes,
    PRIMARY KEY(scope_id,model,revision),
    CHECK ((mode='model_markup')=(percent IS NOT NULL)),
    CHECK ((mode='fixed')=(inputs IS NULL)),
    CHECK (inputs IS NULL OR jsonb_typeof(inputs)='array')
);
CREATE TRIGGER sealed_text_source_markup BEFORE UPDATE OR DELETE ON relay_hosted.text_source_markups
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER sealed_text_sale_review BEFORE UPDATE OR DELETE ON relay_hosted.text_sale_reviews
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE VIEW relay_hosted.latest_text_source_markup AS
    SELECT DISTINCT ON (source_id) source_id,revision,percent
    FROM relay_hosted.text_source_markups ORDER BY source_id,revision DESC;
CREATE VIEW relay_hosted.latest_text_sale_review AS
    SELECT DISTINCT ON (scope_id,model) *
    FROM relay_hosted.text_sale_reviews ORDER BY scope_id,model,revision DESC;
ALTER TABLE relay_hosted.text_offers ADD COLUMN sale_review_revision bigint,
    ADD FOREIGN KEY(scope_id,model,sale_review_revision) REFERENCES relay_hosted.text_sale_reviews;

-- Use admitted cards on the current binding; price review does not depend on
-- queue pressure, monthly capacity, cooldowns or probe health. Prefer the
-- latest admitted observation if a source has been checked again.
CREATE FUNCTION relay_hosted.text_price_inputs(scope uuid,public_model text,inherit boolean)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object('source',i.source_id,'item',i.id,
        'purchase_revision',p.revision,'rates',CASE WHEN p.item IS NOT NULL THEN
            jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,'output',p.output_rate::text) END,
        'default_revision',m.revision,'default_percent',m.percent::text) ORDER BY i.source_id),'[]')
    FROM (
        SELECT DISTINCT ON (t.source_id) t.id,t.source_id,r.purchase_revision
        FROM relay_hosted.text_inventory t JOIN relay_hosted.text_model_rules r ON r.id=t.id
        JOIN relay_hosted.service_control s ON s.qualification_scope_ref=t.scope_id AND s.mode='qualification'
        WHERE t.scope_id=scope AND t.model=public_model AND r.admitted
          AND relay_hosted.text_source_matches(s.installation_id,t.source_id,t.provider_revision,t.proxy_revision,t.uses_proxy)
        ORDER BY t.source_id,t.checked_at DESC,t.id
    ) i LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=i.purchase_revision
    LEFT JOIN relay_hosted.latest_text_source_markup m ON inherit AND m.source_id=i.source_id
$$;

CREATE FUNCTION relay_hosted.owner_text_price_inputs(origin text,tag bytea,item_id uuid,inherit boolean)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN coalesce((SELECT relay_hosted.text_price_inputs(i.scope_id,i.model,inherit)
        FROM relay_hosted.text_inventory i JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        JOIN relay_hosted.installations h ON h.id=s.installation_id WHERE i.id=item_id AND h.public_origin=origin),'[]');
END $$;

CREATE FUNCTION relay_hosted.text_below_cost(scope uuid,public_model text,sale jsonb)
RETURNS boolean LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r ON r.id=i.id
        JOIN relay_hosted.text_purchase_rates p ON p.item=r.id AND p.revision=r.purchase_revision
        JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id AND s.mode='qualification'
        WHERE i.scope_id=scope AND i.model=public_model AND r.admitted
            AND relay_hosted.text_source_matches(s.installation_id,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy)
            AND ((sale->>'input')::numeric<p.input_rate OR (sale->>'cached')::numeric<p.cached_rate
                OR (sale->>'output')::numeric<p.output_rate))
$$;

REVOKE ALL ON FUNCTION relay_hosted.text_price_inputs(uuid,text,boolean),
    relay_hosted.owner_text_price_inputs(text,bytea,uuid,boolean),relay_hosted.text_below_cost(uuid,text,jsonb) FROM PUBLIC;

ALTER TABLE relay_hosted.text_rule_changes DROP CONSTRAINT text_rule_changes_action_check,
    ADD CHECK (action IN ('admit','save','review','publish','suspend'));

CREATE OR REPLACE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,'source',i.source_label,'proxy',i.uses_proxy,
        'revision',r.revision::text,'admitted',r.admitted,'enabled',r.enabled,'ready',c.id IS NOT NULL AND NOT EXISTS(
            SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id WHERE t.inventory_id=i.id),
        'purchase',CASE WHEN p.item IS NOT NULL THEN jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,
            'output',p.output_rate::text) END,
        'source_markup',jsonb_build_object('revision',coalesce(m.revision,0)::text,'percent',m.percent::text),
        'sale_rule',jsonb_build_object('revision',coalesce(q.revision,0)::text,'mode',coalesce(q.mode,'fixed'),
            'percent',q.percent::text,'stale',q.inputs IS NOT NULL AND q.inputs<>relay_hosted.text_price_inputs(i.scope_id,i.model,q.mode='api_default'),
            'below_cost',relay_hosted.text_below_cost(i.scope_id,i.model,coalesce(q.rates,r.draft))),
        'draft',coalesce(q.rates,r.draft),'effective',CASE WHEN l.id IS NOT NULL THEN relay_hosted.text_rates(o) END)
        ORDER BY i.model,i.source_label,i.id),'[]') FROM relay_hosted.text_inventory i
        JOIN relay_hosted.text_model_rules r ON r.id=i.id JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        JOIN relay_hosted.installations h ON h.id=s.installation_id AND h.public_origin=origin
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id
        LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=r.purchase_revision
        LEFT JOIN relay_hosted.latest_text_source_markup m ON m.source_id=i.source_id
        LEFT JOIN relay_hosted.latest_text_sale_review q ON q.scope_id=i.scope_id AND q.model=i.model);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.change_text_rule(origin text,tag bytea,item_id uuid,op uuid,rev bigint,act text,data jsonb)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor_id uuid; proof_at timestamptz; iid uuid; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; review relay_hosted.text_sale_reviews;
    new_id uuid; n bigint; mode text; sale jsonb; cost jsonb; draft jsonb; inputs jsonb; intent jsonb;
    markup jsonb; pct numeric; default_pct numeric; default_rev bigint; source uuid;
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
            sale:=data; -- Preserve sale-only commands and their historical replay format.
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
        ELSIF NOT r.admitted THEN RETURN 'invalid'; END IF;
        IF act<>'review' AND (NOT EXISTS(SELECT FROM relay_hosted.current_text_inventory WHERE id=item_id)
            OR EXISTS(SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id WHERE t.inventory_id=item_id))
            THEN RETURN 'inactive'; END IF;
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
            inputs:=relay_hosted.text_price_inputs(i.scope_id,i.model,mode='api_default');
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
            IF r.purchase_revision IS NULL THEN RETURN 'purchase_required'; END IF;
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
