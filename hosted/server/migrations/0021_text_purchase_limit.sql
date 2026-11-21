-- A connection's policy survives credential rotation, requalification and tariff changes.
CREATE TABLE relay_hosted.text_purchase_limit_changes (
    source_id uuid NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    operation uuid NOT NULL UNIQUE,
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    limit_micro_usd bigint CHECK (limit_micro_usd>=0),
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(source_id,revision)
);
CREATE TRIGGER sealed_purchase_limit_change BEFORE UPDATE OR DELETE ON relay_hosted.text_purchase_limit_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE TABLE relay_hosted.text_purchase_periods (
    source_id uuid NOT NULL,
    month_utc date NOT NULL CHECK (extract(day FROM month_utc)=1),
    accounted_micro_usd numeric NOT NULL DEFAULT 0 CHECK (accounted_micro_usd>=0),
    outstanding_micro_usd numeric NOT NULL DEFAULT 0 CHECK (outstanding_micro_usd>=0),
    unpriced boolean NOT NULL DEFAULT false,
    PRIMARY KEY(source_id,month_utc)
);
-- Rebuild the projection from the attempt journal before installing its writers.
LOCK TABLE relay_hosted.text_requests,relay_hosted.text_purchase_attempts,relay_hosted.text_purchase_results IN SHARE ROW EXCLUSIVE MODE;
INSERT INTO relay_hosted.text_purchase_periods
    SELECT parts.source_id,parts.month_utc,sum(parts.accounted),sum(parts.outstanding),bool_or(parts.unpriced)
    FROM (
        SELECT i.source_id,a.month_utc,coalesce(v.valued_micro_usd,0) AS accounted,
            CASE WHEN v.attempt_id IS NULL THEN a.bound_micro_usd ELSE 0 END AS outstanding,false AS unpriced
        FROM relay_hosted.text_purchase_attempts a JOIN relay_hosted.text_inventory i ON i.id=a.item
        LEFT JOIN relay_hosted.text_purchase_results v USING(attempt_id)
        UNION ALL
        SELECT o.source_id,date_trunc('month',r.claimed_at AT TIME ZONE 'UTC')::date,0,0,true
        FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        WHERE r.claimed_at IS NOT NULL AND NOT EXISTS (
            SELECT FROM relay_hosted.text_purchase_attempts a WHERE a.attempt_id=r.attempt_id)
    ) parts GROUP BY parts.source_id,parts.month_utc;

CREATE FUNCTION relay_hosted.project_text_purchase() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE src uuid; period date; reserved bigint;
BEGIN
    SELECT i.source_id,a.month_utc,a.bound_micro_usd INTO STRICT src,period,reserved
    FROM relay_hosted.text_purchase_attempts a JOIN relay_hosted.text_inventory i ON i.id=a.item
    WHERE a.attempt_id=NEW.attempt_id;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||src::text,0));
    IF TG_TABLE_NAME='text_purchase_attempts' THEN
        INSERT INTO relay_hosted.text_purchase_periods AS p (source_id,month_utc,outstanding_micro_usd)
            VALUES(src,period,reserved)
        ON CONFLICT (source_id,month_utc) DO UPDATE
            SET outstanding_micro_usd=p.outstanding_micro_usd+excluded.outstanding_micro_usd;
    ELSE
        UPDATE relay_hosted.text_purchase_periods p
        SET outstanding_micro_usd=p.outstanding_micro_usd-reserved,
            accounted_micro_usd=p.accounted_micro_usd+NEW.valued_micro_usd
        WHERE p.source_id=src AND p.month_utc=period;
        IF NOT FOUND THEN RAISE EXCEPTION 'purchase period projection mismatch'; END IF;
    END IF;
    RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.project_text_purchase() FROM PUBLIC;
CREATE TRIGGER project_purchase_bound AFTER INSERT ON relay_hosted.text_purchase_attempts
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.project_text_purchase();
CREATE TRIGGER project_purchase_expense AFTER INSERT ON relay_hosted.text_purchase_results
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.project_text_purchase();

CREATE OR REPLACE FUNCTION relay_hosted.claim_text(origin text,oid uuid,rid uuid,aid uuid,space uuid,kid uuid,
    source uuid,model text,auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE proof bytea; o relay_hosted.text_offers; p relay_hosted.text_purchase_rates; req relay_hosted.text_requests;
    stamp timestamptz; period date; purchase_bound bigint;
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
    SELECT v.* INTO p FROM relay_hosted.text_model_rules r JOIN relay_hosted.text_purchase_rates v
        ON v.item=r.id AND v.revision=r.purchase_revision WHERE r.id=o.inventory_id;
    IF NOT FOUND THEN RETURN false; END IF;
    SELECT * INTO req FROM relay_hosted.text_requests r
        WHERE (r.id,r.attempt_id,r.workspace_id,r.key_id,r.offer_id,r.held,r.send_tag,r.output_bound)
            =(rid,aid,space,kid,oid,bound,tag,output_cap) AND r.authority_epoch=auth_rev
            AND r.execution='reserved' FOR UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    stamp:=clock_timestamp();
    IF req.claim_by<=stamp THEN RETURN false; END IF;
    period:=date_trunc('month',stamp AT TIME ZONE 'UTC')::date;
    purchase_bound:=ceil(req.input_bound*greatest(p.input_rate,p.cached_rate)+req.output_bound*p.output_rate)::bigint;
    -- Same source lock as limit changes and expense posting; no lock survives provider I/O.
    IF EXISTS (
        SELECT FROM (SELECT c.limit_micro_usd FROM relay_hosted.text_purchase_limit_changes c
            WHERE c.source_id=source ORDER BY c.revision DESC LIMIT 1) cap
        LEFT JOIN relay_hosted.text_purchase_periods m ON m.source_id=source AND m.month_utc=period
        WHERE cap.limit_micro_usd IS NOT NULL AND (
            coalesce(m.unpriced,false) OR coalesce(m.accounted_micro_usd+m.outstanding_micro_usd,0)+purchase_bound>cap.limit_micro_usd)
    ) THEN RETURN false; END IF;
    UPDATE relay_hosted.text_requests SET execution='dispatched',claimed_at=stamp WHERE id=rid;
    INSERT INTO relay_hosted.text_purchase_attempts(attempt_id,item,rate_revision,month_utc,bound_micro_usd)
        VALUES(aid,p.item,p.revision,period,purchase_bound);
    RETURN true;
END $$;

CREATE FUNCTION relay_hosted.owner_purchase_limit(origin text,tag bytea,source uuid)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE period date;
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    period:=date_trunc('month',clock_timestamp() AT TIME ZONE 'UTC')::date;
    RETURN coalesce((
        SELECT jsonb_build_object('source_id',s.connection_id,'revision',coalesce(cap.revision,0)::text,
            'month_utc',to_char(period,'YYYY-MM-DD'),'limit_micro_usd',cap.limit_micro_usd::text,
            'accounted_micro_usd',coalesce(p.accounted_micro_usd,0)::text,
            'outstanding_micro_usd',coalesce(p.outstanding_micro_usd,0)::text,
            'unpriced',coalesce(p.unpriced,false),
            'remaining_micro_usd',CASE WHEN cap.limit_micro_usd IS NOT NULL AND NOT coalesce(p.unpriced,false) THEN
                greatest(cap.limit_micro_usd-coalesce(p.accounted_micro_usd+p.outstanding_micro_usd,0),0)::text END)
        FROM relay_hosted.installations i JOIN relay_hosted.connection_secret_slots s
            ON s.installation_id=i.id AND s.purpose='provider'
        LEFT JOIN LATERAL (SELECT c.revision,c.limit_micro_usd FROM relay_hosted.text_purchase_limit_changes c
            WHERE c.source_id=s.connection_id ORDER BY c.revision DESC LIMIT 1) cap ON true
        LEFT JOIN relay_hosted.text_purchase_periods p ON p.source_id=s.connection_id AND p.month_utc=period
        WHERE i.public_origin=origin AND s.connection_id=source
    ),'{}'::jsonb);
END $$;

CREATE FUNCTION relay_hosted.change_purchase_limit(origin text,tag bytea,source uuid,op uuid,expected bigint,lim bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor_id uuid; proof_at timestamptz; prior relay_hosted.text_purchase_limit_changes; current_revision bigint;
BEGIN
    IF origin IS NULL OR tag IS NULL OR source IS NULL OR op IS NULL OR expected IS NULL OR expected<0
        OR op='00000000-0000-0000-0000-000000000000' OR lim<0 THEN RETURN 'invalid'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('purchase-limit-operation:'||op::text,0));
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    SELECT a.user_id,ss.created_at INTO actor_id,proof_at FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions ss ON ss.user_id=a.user_id AND ss.credential_id=a.credential_id
        WHERE ss.verifier=tag AND a.public_origin=origin AND ss.installation_epoch=a.installation_epoch AND ss.user_epoch=a.user_epoch
        AND ss.expires_at>clock_timestamp() AND ss.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF ss,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.text_purchase_limit_changes WHERE operation=op;
    IF FOUND THEN RETURN CASE WHEN (prior.source_id,prior.revision-1,prior.actor,prior.limit_micro_usd)
        IS NOT DISTINCT FROM (source,expected,actor_id,lim) THEN 'applied' ELSE 'conflict' END; END IF;
    PERFORM FROM relay_hosted.installations i JOIN relay_hosted.connection_secret_slots s ON s.installation_id=i.id
        WHERE i.public_origin=origin AND s.connection_id=source AND s.purpose='provider' AND s.generation IS NOT NULL
        FOR SHARE OF s;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT max(revision) INTO current_revision FROM relay_hosted.text_purchase_limit_changes WHERE source_id=source;
    IF coalesce(current_revision,0)<>expected THEN RETURN 'conflict'; END IF;
    IF proof_at<=clock_timestamp()-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    INSERT INTO relay_hosted.text_purchase_limit_changes(source_id,revision,operation,actor,limit_micro_usd)
        VALUES(source,expected+1,op,actor_id,lim);
    RETURN 'applied';
END $$;
REVOKE ALL ON FUNCTION relay_hosted.owner_purchase_limit(text,bytea,uuid),
    relay_hosted.change_purchase_limit(text,bytea,uuid,uuid,bigint,bigint) FROM PUBLIC;
