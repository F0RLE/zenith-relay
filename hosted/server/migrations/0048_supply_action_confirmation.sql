ALTER TABLE relay_hosted.text_rule_changes
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    ADD CHECK (confirmed_by_passkey_id IS NULL OR action IN ('admit','publish','suspend'));
ALTER TABLE relay_hosted.text_purchase_limit_changes
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id);

DROP FUNCTION relay_hosted.change_text_rule(text,bytea,uuid,uuid,bigint,text,jsonb);
DROP FUNCTION relay_hosted.change_purchase_limit(text,bytea,uuid,uuid,bigint,bigint);

-- Take the credential row before the vault or source fence. Source copy can
-- hold a credential share lock while waiting for the vault. Advance the
-- revision only after checking the command under its business locks.
CREATE FUNCTION relay_hosted.lock_supply_signer(actor uuid,credential bytea)
RETURNS boolean LANGUAGE sql STRICT SET search_path=pg_catalog AS $$
    SELECT true FROM relay_hosted.admin_credentials k
    WHERE k.user_id=$1 AND k.credential_id=$2 AND k.revoked_at IS NULL
    FOR NO KEY UPDATE
$$;

CREATE FUNCTION relay_hosted.change_text_rule(origin text,tag bytea,item_id uuid,op uuid,rev bigint,act text,data jsonb,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; r relay_hosted.text_model_rules; i relay_hosted.text_inventory;
    prior relay_hosted.text_rule_changes; old relay_hosted.text_offers; review relay_hosted.text_sale_reviews;
    new_id uuid; n bigint; mode text; sale jsonb; cost jsonb; draft jsonb; inputs jsonb; intent jsonb;
    markup jsonb; pct numeric; default_pct numeric; default_rev bigint; source uuid;
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
        -- A selling edit bumps every supplier's rule. Claim locks the live rule
        -- before the supplying rule, so take that same first lock here.
        PERFORM FROM relay_hosted.text_model_rules x
            WHERE (x.scope_id,x.model)=(i.scope_id,i.model) AND x.enabled FOR NO KEY UPDATE;
    END IF;
    SELECT * INTO STRICT r FROM relay_hosted.text_model_rules WHERE id=item_id FOR NO KEY UPDATE;
    IF r.revision<>rev THEN RETURN 'conflict'; END IF;
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

CREATE FUNCTION relay_hosted.change_purchase_limit(origin text,tag bytea,source uuid,op uuid,expected bigint,lim bigint,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; prior relay_hosted.text_purchase_limit_changes; current_revision bigint;
    proof_result text; confirmed uuid; stamp timestamptz;
BEGIN
    IF source IS NULL OR op IS NULL OR op='00000000-0000-0000-0000-000000000000' OR expected IS NULL
        OR expected NOT BETWEEN 0 AND 9223372036854775806 OR lim<0 THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('purchase-limit-operation:'||op::text,0));
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.text_purchase_limit_changes WHERE operation=op;
    IF FOUND THEN RETURN CASE WHEN (prior.source_id,prior.revision-1,prior.actor,prior.limit_micro_usd)
        IS NOT DISTINCT FROM (source,expected,ss.user_id,lim) THEN 'applied' ELSE 'conflict' END; END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true
        THEN RETURN 'passkey_changed'; END IF;
    -- Match admission and expense posting on the physical connection.
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    PERFORM FROM relay_hosted.installations i JOIN relay_hosted.connection_secret_slots s ON s.installation_id=i.id
        WHERE i.public_origin=origin AND s.connection_id=source AND s.purpose='provider' AND s.generation IS NOT NULL
        FOR SHARE OF s;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT max(revision) INTO current_revision FROM relay_hosted.text_purchase_limit_changes WHERE source_id=source;
    IF coalesce(current_revision,0)<>expected THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NULL THEN
        IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
            THEN RETURN 'denied'; END IF;
        RETURN 'fresh_required';
    END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.text_purchase_limit_changes(source_id,revision,operation,actor,limit_micro_usd,recorded_at,
        confirmed_by_passkey_id) VALUES(source,expected+1,op,ss.user_id,lim,stamp,confirmed);
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.lock_supply_signer(uuid,bytea),
    relay_hosted.change_text_rule(text,bytea,uuid,uuid,bigint,text,jsonb,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.change_purchase_limit(text,bytea,uuid,uuid,bigint,bigint,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
