ALTER TABLE relay_hosted.text_completion_timings
    ADD COLUMN generation_ms integer CHECK (generation_ms BETWEEN 1 AND least(30000,elapsed_ms)),
    ADD COLUMN generation_tokens bigint CHECK (generation_tokens BETWEEN 1 AND generation_ms),
    ADD CHECK ((generation_ms IS NULL)=(generation_tokens IS NULL));
CREATE INDEX text_generation_window ON relay_hosted.text_completion_timings(observed_at,request_id)
    INCLUDE (generation_ms,generation_tokens) WHERE generation_ms IS NOT NULL;
CREATE INDEX text_latency_expiry ON relay_hosted.text_completion_timings(observed_at,request_id)
    WHERE generation_ms IS NULL;
DROP INDEX relay_hosted.text_completion_expiry;

CREATE FUNCTION relay_hosted.capture_text_terminal(oid uuid,rid uuid,aid uuid,kind text,inputs bigint,cached bigint,outputs bigint,
    elapsed integer,generated_ms integer,generated_tokens bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=clock_timestamp(); cutoff timestamptz; streamed boolean;
BEGIN
    IF kind IS NULL OR kind NOT IN ('completed','incomplete','failed') OR inputs<0 OR outputs<0
        OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs)) THEN RETURN false; END IF;
    SELECT r.close_by,r.streaming INTO cutoff,streamed FROM relay_hosted.text_requests r WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid)
        AND r.claimed_at IS NOT NULL AND (kind<>'failed' OR r.streaming) FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_terminal_evidence(request_id,outcome,input_tokens,cached_tokens,output_tokens,observed_at)
        VALUES(rid,kind,inputs,cached,outputs,stamp) ON CONFLICT (request_id) DO NOTHING;
    IF FOUND THEN
        IF kind='completed' AND stamp<cutoff AND elapsed BETWEEN 0 AND 90000 THEN
            IF (streamed AND generated_ms BETWEEN 1 AND least(30000,elapsed)
                AND generated_tokens BETWEEN 1 AND generated_ms AND generated_tokens<outputs) IS NOT TRUE THEN
                generated_ms:=NULL;
                generated_tokens:=NULL;
            END IF;
            INSERT INTO relay_hosted.text_completion_timings(request_id,observed_at,elapsed_ms,generation_ms,generation_tokens)
                VALUES(rid,stamp,elapsed,generated_ms,generated_tokens);
        END IF;
        RETURN true;
    END IF;
    RETURN EXISTS(SELECT FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=rid
        AND (e.outcome,e.input_tokens,e.cached_tokens,e.output_tokens) IS NOT DISTINCT FROM (kind,inputs,cached,outputs));
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.capture_text_terminal(oid uuid,rid uuid,aid uuid,kind text,inputs bigint,cached bigint,outputs bigint,elapsed integer)
RETURNS boolean LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT relay_hosted.capture_text_terminal($1,$2,$3,$4,$5,$6,$7,$8,NULL::integer,NULL::bigint)
$$;

CREATE OR REPLACE FUNCTION relay_hosted.prune_model_status_completions()
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH latency AS (
        SELECT request_id FROM relay_hosted.text_completion_timings
        WHERE generation_ms IS NULL AND observed_at<=statement_timestamp()-interval '168 hours'
        ORDER BY observed_at,request_id LIMIT 2048 FOR UPDATE SKIP LOCKED
    ), generation AS (
        SELECT request_id FROM relay_hosted.text_completion_timings
        WHERE generation_ms IS NOT NULL AND observed_at<=statement_timestamp()-interval '720 hours'
        ORDER BY observed_at,request_id LIMIT 2048 FOR UPDATE SKIP LOCKED
    )
    DELETE FROM relay_hosted.text_completion_timings t
    USING (SELECT request_id FROM latency UNION ALL SELECT request_id FROM generation) expired
    WHERE t.request_id=expired.request_id
$$;

CREATE FUNCTION relay_hosted.desktop_pool_capacity(iid uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SET search_path=pg_catalog AS $$
DECLARE scope uuid; stamp timestamptz:=statement_timestamp();
BEGIN
    SELECT s.qualification_scope_ref INTO scope
    FROM relay_hosted.service_control s JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
    WHERE s.installation_id=iid AND s.mode='qualification' AND s.restore_operation IS NULL AND q.expires_at>stamp;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN (WITH members AS (
        SELECT DISTINCT i.source_id
        FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r ON r.id=i.id AND r.admitted
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(iid,i.source_id)
        WHERE i.scope_id=scope AND i.inventory_epoch=coalesce(v.inventory_epoch,0)
    ), routes AS MATERIALIZED (
        SELECT i.source_id,relay_hosted.text_source_state(i.source_id,i.model,
            relay_hosted.text_cost_bound(o.max_input_bytes*o.tokens_per_byte+o.overhead_tokens,o.max_output_tokens,
                i.input_rate,i.cached_rate,i.output_rate),stamp) AS state
        FROM relay_hosted.text_offers o JOIN relay_hosted.text_model_rules r
            ON (r.id,r.last_offer)=(o.inventory_id,o.id) AND r.enabled AND r.admitted
        CROSS JOIN LATERAL relay_hosted.text_offer_supply(o) i
        WHERE o.scope_id=scope
    ), capacity AS (
        SELECT m.source_id,bool_or(a.state='ready') AS ready,bool_or(a.state='busy') AS busy
        FROM members m LEFT JOIN routes a USING(source_id) GROUP BY m.source_id
    )
    SELECT jsonb_build_object('members',count(*)::text,'ready',count(*) FILTER (WHERE ready)::text,
        'busy',count(*) FILTER (WHERE NOT ready AND busy)::text) FROM capacity);
END $$;

CREATE FUNCTION relay_hosted.device_overview_metrics(origin text,iid uuid,device uuid,proof bytea,hours integer)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE snapshot jsonb; stamp timestamptz:=date_trunc('milliseconds',statement_timestamp()); generation jsonb;
BEGIN
    snapshot:=relay_hosted.device_overview(origin,iid,device,proof,hours);
    IF snapshot IS NULL OR snapshot ? 'error' THEN RETURN snapshot; END IF;
    SELECT CASE WHEN count(*)>0 THEN jsonb_build_object('samples',count(*)::text,
        'tokens_per_second',round(sum(generation_tokens)*1000/sum(generation_ms)::numeric,2)) END
    INTO generation FROM relay_hosted.text_completion_timings
    WHERE generation_ms IS NOT NULL AND observed_at>=stamp-hours*interval '1 hour' AND observed_at<stamp;
    RETURN jsonb_set(snapshot,'{result,metrics}',
        jsonb_build_object('pool',relay_hosted.desktop_pool_capacity(iid),'generation',generation));
END $$;

REVOKE ALL ON FUNCTION relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint,integer,integer,bigint),
    relay_hosted.desktop_pool_capacity(uuid),relay_hosted.device_overview_metrics(text,uuid,uuid,bytea,integer) FROM PUBLIC;
