-- Disposable reporting data, never inference or accounting authority.
CREATE TABLE relay_hosted.usage_report_slots (slot smallint PRIMARY KEY);
INSERT INTO relay_hosted.usage_report_slots SELECT generate_series(1,96);

CREATE UNLOGGED TABLE relay_hosted.usage_reports (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slot smallint NOT NULL UNIQUE REFERENCES relay_hosted.usage_report_slots,
    workspace_id uuid NOT NULL REFERENCES relay_hosted.workspaces,
    as_of timestamptz NOT NULL,
    hours integer NOT NULL CHECK (hours IN (24,168,720)),
    expires_at timestamptz NOT NULL DEFAULT clock_timestamp()+interval '10 minutes'
);
CREATE INDEX usage_report_owner ON relay_hosted.usage_reports(workspace_id,expires_at);
CREATE UNLOGGED TABLE relay_hosted.usage_report_receipts (
    report_id uuid NOT NULL REFERENCES relay_hosted.usage_reports ON DELETE CASCADE,
    request_id uuid NOT NULL,
    started_at timestamptz NOT NULL,
    bucket integer NOT NULL CHECK (bucket BETWEEN 0 AND 29),
    model text COLLATE "C" NOT NULL,
    receipt jsonb NOT NULL,
    PRIMARY KEY(report_id,request_id)
);
CREATE INDEX usage_report_order
    ON relay_hosted.usage_report_receipts(report_id,started_at DESC,request_id DESC);
CREATE INDEX usage_report_bucket_order
    ON relay_hosted.usage_report_receipts(report_id,bucket,started_at DESC,request_id DESC);

CREATE FUNCTION relay_hosted.usage_receipt(r relay_hosted.text_requests)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT relay_hosted.text_receipt(r) || jsonb_build_object(
        'started_at_ms',floor(extract(epoch FROM r.started_at)*1000)::bigint,'usage_quality',CASE
        WHEN r.basis='provider_final' THEN 'final'
        WHEN r.input_tokens IS NOT NULL OR r.output_tokens IS NOT NULL THEN 'partial'
        ELSE 'unknown' END)
    || coalesce((SELECT jsonb_build_object('usage_quality','final','input_tokens',l.input_tokens::text,
        'cached_tokens',l.cached_tokens::text,'output_tokens',l.output_tokens::text,
        'total_tokens',(l.input_tokens::numeric+l.output_tokens)::text)
        FROM relay_hosted.text_late_usage l WHERE l.request_id=r.id),'{}')
$$;

-- One statement observes the requests, journal and late evidence at the same
-- MVCC snapshot. The cap detects overflow without copying a partial report.
CREATE FUNCTION relay_hosted.save_usage_report(space uuid, chosen smallint, width integer, anchor timestamptz)
RETURNS uuid LANGUAGE sql SET search_path=pg_catalog AS $$
    WITH bounds AS (
        SELECT anchor-width*interval '1 hour' AS start_at,
            CASE WHEN width=24 THEN 3600 ELSE 86400 END AS seconds
    ), population AS MATERIALIZED (
        SELECT r FROM relay_hosted.text_requests r,bounds b
        WHERE r.workspace_id=space AND r.started_at>=b.start_at AND r.started_at<anchor
        ORDER BY r.started_at DESC,r.id DESC LIMIT 8193
    ), saved AS (
        INSERT INTO relay_hosted.usage_reports(slot,workspace_id,as_of,hours)
            SELECT chosen,space,anchor,width WHERE (SELECT count(*) FROM population)<=8192
            RETURNING id
    ), receipts AS (
        INSERT INTO relay_hosted.usage_report_receipts(report_id,request_id,started_at,bucket,model,receipt)
        SELECT s.id,(p.r).id,(p.r).started_at,floor(extract(epoch FROM (p.r).started_at-b.start_at)/b.seconds)::integer,
            x.receipt->>'model',x.receipt
        FROM saved s CROSS JOIN population p CROSS JOIN bounds b
        CROSS JOIN LATERAL (SELECT relay_hosted.usage_receipt(p.r) AS receipt OFFSET 0) x
    )
    SELECT id FROM saved
$$;

CREATE FUNCTION relay_hosted.start_usage_report(origin text,provider uuid,binding bytea,token bytea,width integer)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; chosen smallint; report uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF width IS NULL OR width NOT IN (24,168,720) THEN RETURN '{"error":"invalid_filter"}'; END IF;
    -- Serialize the per-workspace quota check, not report builds for unrelated workspaces.
    IF NOT pg_try_advisory_xact_lock(hashtextextended(space::text,29029)) THEN RETURN '{"error":"report_busy"}'; END IF;
    IF (SELECT count(*) FROM relay_hosted.usage_reports r WHERE r.workspace_id=space AND r.expires_at>statement_timestamp())>=16
    THEN RETURN '{"error":"report_busy"}'; END IF;
    SELECT s.slot INTO chosen FROM relay_hosted.usage_report_slots s
        WHERE NOT EXISTS (SELECT FROM relay_hosted.usage_reports r WHERE r.slot=s.slot AND r.expires_at>statement_timestamp())
        ORDER BY s.slot LIMIT 1 FOR UPDATE OF s SKIP LOCKED;
    IF NOT FOUND THEN RETURN '{"error":"report_busy"}'; END IF;
    DELETE FROM relay_hosted.usage_reports r WHERE r.slot=chosen AND r.expires_at<=clock_timestamp();
    IF EXISTS (SELECT FROM relay_hosted.usage_reports r WHERE r.slot=chosen) THEN RETURN '{"error":"report_busy"}'; END IF;
    report:=relay_hosted.save_usage_report(space,chosen,width,date_trunc('milliseconds',clock_timestamp()));
    IF report IS NULL THEN RETURN '{"error":"report_too_large"}'; END IF;
    RETURN jsonb_build_object('report_id',report);
END $$;

CREATE FUNCTION relay_hosted.usage_report_metrics(report uuid,model_filter text,bucket_filter integer)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    WITH measured AS (
        SELECT r.bucket,count(*) AS requests,count(r.receipt->>'total_tokens') AS token_samples,
            count(*) FILTER (WHERE r.receipt->>'usage_quality'='partial') AS partial_tokens,
            sum((r.receipt->>'total_tokens')::numeric) AS tokens,
            sum((r.receipt->>'charge_micro_usd')::numeric
                - coalesce((r.receipt->'credit'->>'amount_micro_usd')::numeric,0))
                FILTER (WHERE r.receipt->>'wallet_receipt_id' IS NOT NULL) AS spend,
            count(*) FILTER (WHERE r.receipt->>'charge_micro_usd' IS NULL) AS unsettled
        FROM relay_hosted.usage_report_receipts r
        WHERE r.report_id=report AND (model_filter IS NULL OR r.model=model_filter)
            AND (bucket_filter IS NULL OR r.bucket=bucket_filter)
        GROUP BY GROUPING SETS ((),(r.bucket))
    )
    SELECT jsonb_object_agg(coalesce(bucket,-1),jsonb_build_object(
        'requests',requests,'token_samples',token_samples,'partial_tokens',partial_tokens,
        'total_tokens',CASE WHEN requests=0 THEN '0' ELSE tokens::text END,
        'spend_micro_usd',coalesce(spend,0)::text,'unsettled',unsettled))
    FROM measured
$$;

CREATE FUNCTION relay_hosted.usage_report_payload(h relay_hosted.usage_reports,model_filter text,bucket_filter integer,
    before_id uuid,target uuid,overview boolean)
RETURNS jsonb LANGUAGE plpgsql STABLE SET search_path=pg_catalog AS $$
DECLARE step integer:=CASE h.hours WHEN 24 THEN 1 ELSE 24 END; n integer:=h.hours/step;
    start_at timestamptz:=h.as_of-h.hours*interval '1 hour'; cursor_at timestamptz;
    metrics jsonb; result jsonb; detail jsonb;
    empty_bucket jsonb:='{"requests":0,"token_samples":0,"partial_tokens":0,"total_tokens":"0","spend_micro_usd":"0","unsettled":0}';
BEGIN
    IF bucket_filter IS NOT NULL AND bucket_filter NOT BETWEEN 0 AND n-1 THEN
        RETURN '{"error":"invalid_filter"}';
    END IF;
    IF before_id IS NOT NULL THEN
        SELECT r.started_at INTO cursor_at FROM relay_hosted.usage_report_receipts r
            WHERE r.report_id=h.id AND r.request_id=before_id AND (model_filter IS NULL OR r.model=model_filter)
                AND (bucket_filter IS NULL OR r.bucket=bucket_filter);
        IF NOT FOUND THEN RETURN '{"error":"invalid_filter"}'; END IF;
    END IF;
    metrics:=relay_hosted.usage_report_metrics(h.id,model_filter,bucket_filter);
    result:=jsonb_build_object(
        'report_id',h.id,'hours',h.hours,'as_of_ms',(extract(epoch FROM h.as_of)*1000)::bigint,
        'from_ms',(extract(epoch FROM (start_at+coalesce(bucket_filter*step,0)*interval '1 hour'))*1000)::bigint,
        'to_ms',(extract(epoch FROM (CASE WHEN bucket_filter IS NULL THEN h.as_of
            ELSE start_at+(bucket_filter+1)*step*interval '1 hour' END))*1000)::bigint,
        'model',model_filter,'bucket',bucket_filter,'coverage','complete','currency','USD','precision',6,'totals',metrics->'-1');
    IF overview THEN
        SELECT jsonb_agg(coalesce(metrics->i::text,empty_bucket) ORDER BY i) INTO detail FROM generate_series(0,n-1) i;
        RETURN result || jsonb_build_object('buckets',detail,'models',coalesce(
            (SELECT jsonb_agg(DISTINCT r.model ORDER BY r.model) FROM relay_hosted.usage_report_receipts r WHERE r.report_id=h.id),'[]'));
    END IF;
    SELECT coalesce(jsonb_agg(p.receipt ORDER BY p.started_at DESC,p.request_id DESC),'[]') INTO detail
    FROM (SELECT r.receipt,r.started_at,r.request_id FROM relay_hosted.usage_report_receipts r
        WHERE r.report_id=h.id AND (model_filter IS NULL OR r.model=model_filter)
            AND (bucket_filter IS NULL OR r.bucket=bucket_filter) AND (target IS NULL OR r.request_id=target)
            AND (before_id IS NULL OR (r.started_at,r.request_id)<(cursor_at,before_id))
        ORDER BY r.started_at DESC,r.request_id DESC LIMIT 33) p;
    RETURN result || jsonb_build_object('items',detail);
END $$;

CREATE FUNCTION relay_hosted.read_usage_report(origin text,provider uuid,binding bytea,token bytea,report uuid,
    model_filter text,bucket_filter integer,before_id uuid,target uuid,overview boolean)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; result jsonb;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF overview IS NULL OR (overview AND (bucket_filter IS NOT NULL OR before_id IS NOT NULL OR target IS NOT NULL))
        OR (before_id IS NOT NULL AND target IS NOT NULL)
        OR (model_filter IS NOT NULL AND (model_filter COLLATE "C") !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
    THEN RETURN '{"error":"invalid_filter"}'; END IF;
    IF report IS NULL AND NOT overview AND target IS NOT NULL AND model_filter IS NULL AND bucket_filter IS NULL THEN
        SELECT jsonb_build_object('items',coalesce(jsonb_agg(relay_hosted.usage_receipt(r)),'[]')) INTO result
        FROM relay_hosted.text_requests r WHERE r.workspace_id=space AND r.id=target;
        RETURN result;
    END IF;
    -- The stable renderer sees the same snapshot as this header lookup, even
    -- if cleanup reclaims the slot while rendering the response.
    SELECT relay_hosted.usage_report_payload(h,model_filter,bucket_filter,before_id,target,overview) INTO result
    FROM relay_hosted.usage_reports h WHERE h.id=report AND h.workspace_id=space AND h.expires_at>clock_timestamp();
    RETURN coalesce(result,'{"error":"report_not_found"}');
END $$;

CREATE FUNCTION relay_hosted.prune_usage_reports()
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH expired AS MATERIALIZED (
        SELECT s.slot FROM relay_hosted.usage_report_slots s JOIN relay_hosted.usage_reports r USING(slot)
        WHERE r.expires_at<=statement_timestamp() ORDER BY r.expires_at LIMIT 8 FOR UPDATE OF s SKIP LOCKED
    )
    DELETE FROM relay_hosted.usage_reports r USING expired e
        WHERE r.slot=e.slot AND r.expires_at<=statement_timestamp()
$$;

REVOKE ALL ON FUNCTION relay_hosted.usage_receipt(relay_hosted.text_requests),
    relay_hosted.save_usage_report(uuid,smallint,integer,timestamptz),
    relay_hosted.usage_report_metrics(uuid,text,integer),
    relay_hosted.usage_report_payload(relay_hosted.usage_reports,text,integer,uuid,uuid,boolean),
    relay_hosted.start_usage_report(text,uuid,bytea,bytea,integer),
    relay_hosted.read_usage_report(text,uuid,bytea,bytea,uuid,text,integer,uuid,uuid,boolean),
    relay_hosted.prune_usage_reports() FROM PUBLIC;
