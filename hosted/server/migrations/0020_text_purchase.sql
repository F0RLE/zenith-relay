-- Seller expenses follow the physical permit, not the customer's tariff or wallet.
CREATE TABLE relay_hosted.text_purchase_rates (
    item uuid NOT NULL REFERENCES relay_hosted.text_inventory,
    revision bigint NOT NULL CHECK (revision>0),
    input_rate numeric(30,12) NOT NULL CHECK (input_rate>=0 AND input_rate<1e12),
    cached_rate numeric(30,12) NOT NULL CHECK (cached_rate>=0 AND cached_rate<1e12),
    output_rate numeric(30,12) NOT NULL CHECK (output_rate>=0 AND output_rate<1e12),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(item,revision)
);
ALTER TABLE relay_hosted.text_model_rules ADD COLUMN purchase_revision bigint,
    ADD FOREIGN KEY(id,purchase_revision) REFERENCES relay_hosted.text_purchase_rates(item,revision);
-- An old listing has no known purchase rates. Do not price it as zero or carry activation forward.
UPDATE relay_hosted.text_model_rules SET enabled=false,revision=revision+1 WHERE enabled;
ALTER TABLE relay_hosted.text_model_rules ADD CHECK (NOT enabled OR purchase_revision IS NOT NULL);

CREATE TABLE relay_hosted.text_purchase_attempts (
    attempt_id uuid PRIMARY KEY REFERENCES relay_hosted.text_requests(attempt_id),
    item uuid NOT NULL,
    rate_revision bigint NOT NULL,
    month_utc date NOT NULL CHECK (extract(day FROM month_utc)=1),
    bound_micro_usd bigint NOT NULL CHECK (bound_micro_usd>=0),
    FOREIGN KEY(item,rate_revision) REFERENCES relay_hosted.text_purchase_rates
);
CREATE INDEX text_purchase_month ON relay_hosted.text_purchase_attempts(item,month_utc);

CREATE TABLE relay_hosted.text_purchase_results (
    attempt_id uuid PRIMARY KEY REFERENCES relay_hosted.text_purchase_attempts,
    input_tokens bigint NOT NULL CHECK (input_tokens>=0),
    cached_tokens bigint CHECK (cached_tokens BETWEEN 0 AND input_tokens),
    output_tokens bigint NOT NULL CHECK (output_tokens>=0),
    valued_micro_usd numeric(40,0) NOT NULL CHECK (valued_micro_usd>=0),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER sealed_purchase_rate BEFORE UPDATE OR DELETE ON relay_hosted.text_purchase_rates
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER sealed_purchase_attempt BEFORE UPDATE OR DELETE ON relay_hosted.text_purchase_attempts
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER sealed_purchase_result BEFORE UPDATE OR DELETE ON relay_hosted.text_purchase_results
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE OR REPLACE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,'source',i.source_label,'proxy',i.uses_proxy,
        'revision',r.revision::text,'admitted',r.admitted,'enabled',r.enabled,'ready',c.id IS NOT NULL AND NOT EXISTS(
            SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers t ON t.id=f.offer_id WHERE t.inventory_id=i.id),
        'purchase',CASE WHEN p.item IS NOT NULL THEN jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,
            'output',p.output_rate::text) END,
        'draft',r.draft,'effective',CASE WHEN l.id IS NOT NULL THEN relay_hosted.text_rates(o) END)
        ORDER BY i.model,i.source_label,i.id),'[]') FROM relay_hosted.text_inventory i
        JOIN relay_hosted.text_model_rules r ON r.id=i.id JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id
        LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=r.purchase_revision);
END $$;

CREATE FUNCTION relay_hosted.valid_text_rates(prices jsonb)
RETURNS boolean LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
    SELECT CASE WHEN jsonb_typeof(prices)='object' THEN
        (prices ?& ARRAY['input','cached','output']) AND (prices-ARRAY['input','cached','output'])='{}'::jsonb
        AND NOT EXISTS(SELECT FROM jsonb_each(prices) p WHERE jsonb_typeof(p.value)<>'string'
            OR p.value #>> '{}' !~ '^(0|[1-9][0-9]{0,11})\.[0-9]{12}$')
    ELSE false END
$$;
REVOKE ALL ON FUNCTION relay_hosted.valid_text_rates(jsonb) FROM PUBLIC;

CREATE OR REPLACE FUNCTION relay_hosted.change_text_rule(origin text, tag bytea, item_id uuid, op uuid, rev bigint, act text, data jsonb)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor_id uuid; proof_at timestamptz; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; new_id uuid; n bigint; sale jsonb; cost jsonb;
BEGIN
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
        IF NOT r.admitted OR jsonb_typeof(data)<>'object' THEN RETURN 'invalid'; END IF;
        IF relay_hosted.valid_text_rates(data) THEN
            sale:=data; -- Keep sale-only save/replay compatible.
        ELSIF data ? 'purchase' AND (data-ARRAY['sale','purchase'])='{}'::jsonb THEN
            cost:=data->'purchase';
            sale:=data->'sale';
            IF NOT relay_hosted.valid_text_rates(cost) OR (sale IS NOT NULL AND NOT relay_hosted.valid_text_rates(sale))
                THEN RETURN 'invalid'; END IF;
            n:=coalesce(r.purchase_revision,0)+1;
            INSERT INTO relay_hosted.text_purchase_rates(item,revision,input_rate,cached_rate,output_rate)
                VALUES(i.id,n,(cost->>'input')::numeric,(cost->>'cached')::numeric,(cost->>'output')::numeric);
        ELSE RETURN 'invalid';
        END IF;
        UPDATE relay_hosted.text_model_rules SET draft=coalesce(sale,draft),purchase_revision=coalesce(n,purchase_revision) WHERE id=item_id;
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
            IF r.purchase_revision IS NULL THEN RETURN 'purchase_required'; END IF;
            IF r.enabled AND r.draft=(SELECT relay_hosted.text_rates(t) FROM relay_hosted.text_offers t WHERE t.id=r.last_offer)
                THEN RETURN 'conflict'; END IF;
            IF EXISTS(SELECT FROM relay_hosted.text_model_rules x WHERE x.scope_id=i.scope_id AND x.model=i.model AND x.id<>i.id AND x.enabled)
                THEN RETURN 'conflict'; END IF;
            SELECT * INTO old FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model AND t.inventory_id IS NOT NULL
                ORDER BY t.revision DESC LIMIT 1;
            IF FOUND AND ((r.draft->>'input')::numeric>old.input_rate OR (r.draft->>'cached')::numeric>old.cached_rate
                OR (r.draft->>'output')::numeric>old.output_rate) THEN RETURN 'notice_required'; END IF;
            n:=coalesce(old.revision,(SELECT max(t.revision) FROM relay_hosted.text_offers t WHERE t.scope_id=i.scope_id AND t.model=i.model),0)+1;
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

CREATE OR REPLACE FUNCTION relay_hosted.claim_text(origin text,oid uuid,rid uuid,aid uuid,space uuid,kid uuid,
    source uuid,model text,auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE proof bytea; o relay_hosted.text_offers; p relay_hosted.text_purchase_rates; req relay_hosted.text_requests;
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid FOR NO KEY UPDATE;
    IF NOT FOUND OR (o.source_id,o.model,o.revision,o.max_input_bytes)<>(source,model,tariff_rev,input_cap)
        OR EXISTS(SELECT FROM relay_hosted.text_fences WHERE offer_id=oid) THEN RETURN false; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    SELECT verifier INTO proof FROM relay_hosted.customer_keys k WHERE k.id=kid AND k.workspace_id=space;
    IF proof IS NULL OR relay_hosted.lock_text_authority(origin,oid,kid,proof) IS DISTINCT FROM space THEN RETURN false; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id AND s.admission_epoch=auth_rev)
        OR EXISTS(SELECT FROM relay_hosted.text_requests x JOIN relay_hosted.text_offers a ON a.id=x.offer_id
            WHERE a.source_id=source AND x.execution IN ('dispatched','uncertain') AND x.closed_at IS NULL)
        THEN RETURN false; END IF;
    -- lock_text_authority pins the rule while a concurrent owner edit waits.
    SELECT v.* INTO p FROM relay_hosted.text_model_rules r JOIN relay_hosted.text_purchase_rates v
        ON v.item=r.id AND v.revision=r.purchase_revision WHERE r.id=o.inventory_id;
    IF NOT FOUND THEN RETURN false; END IF;
    UPDATE relay_hosted.text_requests r SET execution='dispatched',claimed_at=clock_timestamp()
        WHERE (r.id,r.attempt_id,r.workspace_id,r.key_id,r.offer_id,r.held,r.send_tag,r.output_bound)
            =(rid,aid,space,kid,oid,bound,tag,output_cap) AND r.authority_epoch=auth_rev
            AND r.execution='reserved' AND r.claim_by>clock_timestamp() RETURNING r.* INTO req;
    IF NOT FOUND THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_purchase_attempts(attempt_id,item,rate_revision,month_utc,bound_micro_usd)
        VALUES(aid,p.item,p.revision,date_trunc('month',req.claimed_at AT TIME ZONE 'UTC')::date,
            ceil(req.input_bound*greatest(p.input_rate,p.cached_rate)+req.output_bound*p.output_rate)::bigint);
    RETURN true;
END $$;

CREATE FUNCTION relay_hosted.value_text_purchase(oid uuid, aid uuid, inputs bigint, cached bigint, outputs bigint)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE owed numeric; ceiling bigint;
BEGIN
    SELECT ceil((inputs-coalesce(cached,0))*p.input_rate+coalesce(cached,0)*p.cached_rate+outputs*p.output_rate), a.bound_micro_usd
        INTO owed,ceiling FROM relay_hosted.text_purchase_attempts a JOIN relay_hosted.text_purchase_rates p
        ON (p.item,p.revision)=(a.item,a.rate_revision)
        WHERE a.attempt_id=aid AND inputs>=0 AND outputs>=0 AND (cached BETWEEN 0 AND inputs OR
            (cached IS NULL AND (inputs=0 OR p.input_rate=p.cached_rate)));
    IF NOT FOUND THEN RETURN; END IF;
    INSERT INTO relay_hosted.text_purchase_results(attempt_id,input_tokens,cached_tokens,output_tokens,valued_micro_usd)
        VALUES(aid,inputs,cached,outputs,owed) ON CONFLICT (attempt_id) DO NOTHING;
    IF FOUND AND owed>ceiling THEN
        INSERT INTO relay_hosted.text_fences(offer_id) VALUES(oid) ON CONFLICT DO NOTHING;
    END IF;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.value_text_purchase(uuid,uuid,bigint,bigint,bigint) FROM PUBLIC;

CREATE OR REPLACE FUNCTION relay_hosted.finish_text(oid uuid,rid uuid,aid uuid,outcome text,inputs bigint,cached bigint,outputs bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.text_requests; o relay_hosted.text_offers; final_usage boolean; priceable boolean;
BEGIN
    PERFORM FROM relay_hosted.wallets w JOIN relay_hosted.text_requests x ON x.workspace_id=w.workspace_id
        WHERE x.id=rid AND x.attempt_id=aid AND x.offer_id=oid FOR NO KEY UPDATE OF w;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=oid FOR NO KEY UPDATE;
    SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid FOR UPDATE;
    -- A sealed customer receipt does not erase outstanding supplier exposure.
    IF r.claimed_at IS NOT NULL AND outcome IN ('completed','incomplete') THEN
        PERFORM relay_hosted.value_text_purchase(oid,aid,inputs,cached,outputs);
    END IF;
    IF r.closed_at IS NOT NULL THEN RETURN r.accounting; END IF;
    IF r.execution='reserved' THEN
        IF outcome='not_sent' OR (outcome='deadline' AND r.claim_by<=clock_timestamp()) THEN
            PERFORM relay_hosted.close_text(r,'failed','not_sent'); RETURN 'released'; END IF;
        RETURN 'held';
    END IF;
    IF outcome='deadline' AND r.close_by>clock_timestamp() THEN RETURN r.accounting; END IF;
    IF outcome NOT IN ('completed','incomplete','unknown','not_sent','deadline') OR outcome IS NULL THEN RETURN 'invalid'; END IF;
    IF outcome IN ('completed','incomplete') AND r.close_by>clock_timestamp() AND r.execution IN ('dispatched','uncertain') THEN
        UPDATE relay_hosted.text_requests SET execution=outcome,input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
            WHERE id=rid RETURNING * INTO r;
    ELSIF r.execution='dispatched' THEN
        UPDATE relay_hosted.text_requests SET execution='uncertain',accounting='pending' WHERE id=rid RETURNING * INTO r;
    END IF;
    final_usage:=r.input_tokens IS NOT NULL AND (r.cached_tokens IS NOT NULL OR o.input_rate=o.cached_rate) AND r.output_tokens IS NOT NULL;
    priceable:=r.output_tokens IS NOT NULL OR (r.input_tokens IS NOT NULL AND (o.input_rate=o.cached_rate OR r.cached_tokens IS NOT NULL));
    IF final_usage THEN PERFORM relay_hosted.close_text(r,r.execution,'provider_final'); RETURN 'settled'; END IF;
    IF r.close_by<=clock_timestamp() THEN
        PERFORM relay_hosted.close_text(r,r.execution,CASE WHEN priceable THEN 'provider_partial' ELSE 'policy_waiver' END);
        RETURN CASE WHEN priceable THEN 'settled' ELSE 'released' END;
    END IF;
    RETURN 'pending';
END $$;
