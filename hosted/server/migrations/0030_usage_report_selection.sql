DROP FUNCTION relay_hosted.read_usage_report(text,uuid,bytea,bytea,uuid,text,integer,uuid,uuid,boolean);
DROP FUNCTION relay_hosted.usage_report_payload(relay_hosted.usage_reports,text,integer,uuid,uuid,boolean);
DROP FUNCTION relay_hosted.usage_report_metrics(uuid,text,integer);

CREATE TYPE relay_hosted.usage_filter AS (
    model text, bucket integer, key_id uuid, execution text
);

CREATE FUNCTION relay_hosted.usage_report_rows(report uuid,f relay_hosted.usage_filter)
RETURNS SETOF relay_hosted.usage_report_receipts LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT r.* FROM relay_hosted.usage_report_receipts r WHERE r.report_id=report
        AND ((f).model IS NULL OR r.model=(f).model) AND ((f).bucket IS NULL OR r.bucket=(f).bucket)
        AND ((f).key_id IS NULL OR r.receipt->>'key_id'=(f).key_id::text)
        AND ((f).execution IS NULL OR r.receipt->>'execution'=(f).execution)
$$;

CREATE FUNCTION relay_hosted.usage_report_metrics(report uuid,f relay_hosted.usage_filter)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    WITH measured AS (
        SELECT r.bucket,count(*) AS requests,count(r.receipt->>'total_tokens') AS token_samples,
            count(*) FILTER (WHERE r.receipt->>'usage_quality'='partial') AS partial_tokens,
            sum((r.receipt->>'total_tokens')::numeric) AS tokens,
            sum((r.receipt->>'charge_micro_usd')::numeric
                - coalesce((r.receipt->'credit'->>'amount_micro_usd')::numeric,0))
                FILTER (WHERE r.receipt->>'wallet_receipt_id' IS NOT NULL) AS spend,
            count(*) FILTER (WHERE r.receipt->>'charge_micro_usd' IS NULL) AS unsettled
        FROM relay_hosted.usage_report_rows(report,f) r GROUP BY GROUPING SETS ((),(r.bucket))
    )
    SELECT jsonb_object_agg(coalesce(bucket,-1),jsonb_build_object(
        'requests',requests,'token_samples',token_samples,'partial_tokens',partial_tokens,
        'total_tokens',CASE WHEN requests=0 THEN '0' ELSE tokens::text END,
        'spend_micro_usd',coalesce(spend,0)::text,'unsettled',unsettled))
    FROM measured
$$;

CREATE FUNCTION relay_hosted.usage_report_keys(h relay_hosted.usage_reports)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    -- Labels do not change on revocation/replacement; choices come from the
    -- saved requests, not the current list of usable keys.
    SELECT coalesce(jsonb_agg(jsonb_build_object('id',k.id,'name',k.label,
        'prefix','rlk_'||left(replace(k.id::text,'-',''),12)) ORDER BY k.label COLLATE "C",k.id),'[]')
    FROM (SELECT DISTINCT (r.receipt->>'key_id')::uuid AS id
        FROM relay_hosted.usage_report_receipts r WHERE r.report_id=(h).id) ids
    JOIN relay_hosted.customer_keys k ON k.id=ids.id AND k.workspace_id=(h).workspace_id
$$;

CREATE FUNCTION relay_hosted.usage_report_utc(stamp timestamptz)
RETURNS text LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT to_char(stamp AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.MS"Z"')
$$;

CREATE FUNCTION relay_hosted.usage_report_payload(h relay_hosted.usage_reports,f relay_hosted.usage_filter,
    before_id uuid,target uuid,view_kind text)
RETURNS jsonb LANGUAGE plpgsql STABLE SET search_path=pg_catalog AS $$
DECLARE step integer:=CASE h.hours WHEN 24 THEN 1 ELSE 24 END; n integer:=h.hours/step;
    lower_at timestamptz:=h.as_of-h.hours*interval '1 hour'; upper_at timestamptz:=h.as_of; cursor_at timestamptz;
    metrics jsonb; result jsonb; detail jsonb;
    empty_bucket jsonb:='{"requests":0,"token_samples":0,"partial_tokens":0,"total_tokens":"0","spend_micro_usd":"0","unsettled":0}';
BEGIN
    IF f.bucket IS NOT NULL THEN
        IF f.bucket NOT BETWEEN 0 AND n-1 THEN RETURN '{"error":"invalid_filter"}'; END IF;
        lower_at:=lower_at+f.bucket*step*interval '1 hour';
        upper_at:=lower_at+step*interval '1 hour';
    END IF;
    IF before_id IS NOT NULL THEN
        SELECT r.started_at INTO cursor_at FROM relay_hosted.usage_report_rows(h.id,f) r WHERE r.request_id=before_id;
        IF NOT FOUND THEN RETURN '{"error":"invalid_filter"}'; END IF;
    END IF;
    metrics:=relay_hosted.usage_report_metrics(h.id,f);
    result:=jsonb_build_object(
        'report_id',h.id,'hours',h.hours,'as_of_ms',(extract(epoch FROM h.as_of)*1000)::bigint,
        'from_ms',(extract(epoch FROM lower_at)*1000)::bigint,'to_ms',(extract(epoch FROM upper_at)*1000)::bigint,
        'model',f.model,'bucket',f.bucket,'key_id',f.key_id,'execution',f.execution,
        'coverage','complete','currency','USD','precision',6,'totals',metrics->'-1');
    IF view_kind='overview' THEN
        SELECT jsonb_agg(coalesce(metrics->i::text,empty_bucket) ORDER BY i) INTO detail FROM generate_series(0,n-1) i;
        RETURN result || jsonb_build_object('buckets',detail,'keys',relay_hosted.usage_report_keys(h),'models',coalesce(
            (SELECT jsonb_agg(DISTINCT r.model ORDER BY r.model) FROM relay_hosted.usage_report_receipts r WHERE r.report_id=h.id),'[]'));
    ELSIF view_kind='export' THEN
        result:=result || jsonb_build_object('from_utc',relay_hosted.usage_report_utc(lower_at),
            'to_utc',relay_hosted.usage_report_utc(upper_at),'as_of_utc',relay_hosted.usage_report_utc(h.as_of));
    END IF;
    SELECT coalesce(jsonb_agg(p.receipt || CASE WHEN view_kind='export' THEN
        jsonb_build_object('started_at_utc',relay_hosted.usage_report_utc(p.started_at)) ELSE '{}' END
        ORDER BY p.started_at DESC,p.request_id DESC),'[]') INTO detail
    FROM (SELECT r.receipt,r.started_at,r.request_id FROM relay_hosted.usage_report_rows(h.id,f) r
        WHERE (target IS NULL OR r.request_id=target)
            AND (before_id IS NULL OR (r.started_at,r.request_id)<(cursor_at,before_id))
        ORDER BY r.started_at DESC,r.request_id DESC LIMIT CASE WHEN view_kind='export' THEN 8193 ELSE 33 END) p;
    RETURN result || jsonb_build_object('items',detail);
END $$;

CREATE FUNCTION relay_hosted.read_usage_report(origin text,provider uuid,binding bytea,token bytea,report uuid,
    f relay_hosted.usage_filter,before_id uuid,target uuid,view_kind text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; result jsonb;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF view_kind IS NULL OR view_kind NOT IN ('overview','page','export')
        OR (view_kind<>'page' AND (report IS NULL OR before_id IS NOT NULL OR target IS NOT NULL))
        OR (view_kind='overview' AND (f.bucket IS NOT NULL OR f.key_id IS NOT NULL OR f.execution IS NOT NULL))
        OR (before_id IS NOT NULL AND target IS NOT NULL)
        OR (f.model IS NOT NULL AND (f.model COLLATE "C") !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
        OR (f.execution IS NOT NULL AND f.execution NOT IN ('reserved','dispatched','completed','incomplete','failed','uncertain'))
    THEN RETURN '{"error":"invalid_filter"}'; END IF;
    IF report IS NULL THEN
        IF target IS NULL OR before_id IS NOT NULL OR NOT (f IS NULL) THEN RETURN '{"error":"invalid_filter"}'; END IF;
        SELECT jsonb_build_object('items',coalesce(jsonb_agg(relay_hosted.usage_receipt(r)),'[]')) INTO result
        FROM relay_hosted.text_requests r WHERE r.workspace_id=space AND r.id=target;
        RETURN result;
    END IF;
    SELECT relay_hosted.usage_report_payload(h,f,before_id,target,view_kind) INTO result
    FROM relay_hosted.usage_reports h WHERE h.id=report AND h.workspace_id=space AND h.expires_at>clock_timestamp();
    RETURN coalesce(result,'{"error":"report_not_found"}');
END $$;

REVOKE ALL ON FUNCTION relay_hosted.usage_report_rows(uuid,relay_hosted.usage_filter),
    relay_hosted.usage_report_metrics(uuid,relay_hosted.usage_filter), relay_hosted.usage_report_keys(relay_hosted.usage_reports),
    relay_hosted.usage_report_utc(timestamptz),
    relay_hosted.usage_report_payload(relay_hosted.usage_reports,relay_hosted.usage_filter,uuid,uuid,text),
    relay_hosted.read_usage_report(text,uuid,bytea,bytea,uuid,relay_hosted.usage_filter,uuid,uuid,text) FROM PUBLIC;
