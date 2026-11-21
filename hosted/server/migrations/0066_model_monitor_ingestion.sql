CREATE INDEX model_status_observation_expiry ON relay_hosted.model_status_observations(slot_at,publication_id);

CREATE FUNCTION relay_hosted.model_monitor_targets(origin text)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH targets AS (
        SELECT p.id,p.model,p.created_at FROM relay_hosted.installations h
        JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        JOIN relay_hosted.text_model_rules r ON r.scope_id=q.id AND r.admitted AND r.enabled
        JOIN relay_hosted.text_inventory i ON i.id=r.id
        JOIN relay_hosted.text_offers p ON p.inventory_id=r.id AND (p.scope_id,p.source_id,p.model,p.revision)=
            (r.scope_id,i.source_id,r.model,r.first_live_revision)
        -- Monitor configured publications even when no supplier is currently ready.
        WHERE h.public_origin=$1 AND s.mode='qualification' AND s.restore_operation IS NULL
            AND q.expires_at>statement_timestamp()
        ORDER BY p.model LIMIT 65
    )
    SELECT CASE WHEN count(*)>64 THEN '{"error":"status_capacity"}'::jsonb ELSE jsonb_build_object('items',
        coalesce(jsonb_agg(jsonb_build_object('publication_id',id,'model',model,
            'published_at_ms',ceil(extract(epoch FROM created_at)*1000)::bigint) ORDER BY model),'[]')) END
    FROM targets
$$;

CREATE FUNCTION relay_hosted.attest_model_status(origin text,publication uuid,slot timestamptz,condition text)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope uuid; scope_since timestamptz; scope_until timestamptz;
    published_at timestamptz; stamp timestamptz; recorded text;
BEGIN
    IF origin IS NULL OR publication IS NULL OR slot IS NULL OR NOT isfinite(slot)
        OR condition IS NULL OR condition NOT IN ('operational','degraded','unavailable') THEN RETURN 'invalid'; END IF;
    SELECT q.id,q.created_at,q.expires_at INTO scope,scope_since,scope_until
    FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
    JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
    WHERE h.public_origin=origin AND s.mode='qualification' AND s.restore_operation IS NULL FOR SHARE OF s;
    IF NOT FOUND THEN RETURN 'inactive'; END IF;
    SELECT p.created_at INTO published_at
    FROM relay_hosted.text_offers p JOIN relay_hosted.text_model_rules r ON (p.inventory_id,p.scope_id,p.model,p.revision)=
        (r.id,r.scope_id,r.model,r.first_live_revision)
    JOIN relay_hosted.text_inventory i ON i.id=r.id AND i.source_id=p.source_id
    WHERE p.id=publication AND p.scope_id=scope AND r.admitted AND r.enabled FOR SHARE OF r;
    IF NOT FOUND THEN RETURN 'inactive'; END IF;
    stamp:=clock_timestamp();
    IF scope_until<=stamp THEN RETURN 'inactive'; END IF;
    IF slot<greatest(scope_since,published_at) OR slot>stamp-interval '30 minutes'
        OR slot<=stamp-interval '168 hours 30 minutes'
        THEN RETURN 'invalid'; END IF;
    IF slot<>date_bin(interval '30 minutes',slot,timestamptz '2000-01-01 00:00:00+00') THEN RETURN 'invalid'; END IF;
    -- A late retry can recover a retained receipt without filling an old gap.
    INSERT INTO relay_hosted.model_status_observations(publication_id,slot_at,state)
        SELECT publication,slot,condition WHERE slot>stamp-interval '90 minutes'
        ON CONFLICT (publication_id,slot_at) DO NOTHING;
    IF FOUND THEN RETURN 'recorded'; END IF;
    SELECT o.state INTO recorded FROM relay_hosted.model_status_observations o
        WHERE (o.publication_id,o.slot_at)=(publication,slot);
    IF NOT FOUND THEN RETURN 'invalid'; END IF;
    RETURN CASE WHEN recorded=condition THEN 'recorded' ELSE 'conflict' END;
END $$;

CREATE FUNCTION relay_hosted.prune_model_status_observations()
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    DELETE FROM relay_hosted.model_status_observations o USING (
        SELECT publication_id,slot_at FROM relay_hosted.model_status_observations
        WHERE slot_at<=statement_timestamp()-interval '168 hours 30 minutes'
        ORDER BY slot_at,publication_id LIMIT 4096 FOR UPDATE SKIP LOCKED
    ) expired WHERE (o.publication_id,o.slot_at)=(expired.publication_id,expired.slot_at)
$$;

REVOKE ALL ON FUNCTION relay_hosted.model_monitor_targets(text),
    relay_hosted.attest_model_status(text,uuid,timestamptz,text),
    relay_hosted.prune_model_status_observations() FROM PUBLIC;
