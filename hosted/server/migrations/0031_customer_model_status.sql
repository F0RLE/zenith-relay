-- Only a trusted monitor may attest a completed UTC half-hour. Catalog
-- qualification and customer execution never write this table.
CREATE TABLE relay_hosted.model_status_observations (
    publication_id uuid NOT NULL REFERENCES relay_hosted.text_offers,
    slot_at timestamptz NOT NULL CHECK (isfinite(slot_at) AND slot_at=
        date_bin(interval '30 minutes',slot_at,timestamptz '2000-01-01 00:00:00+00')),
    state text NOT NULL CHECK (state IN ('operational','degraded','unavailable')),
    PRIMARY KEY(publication_id,slot_at)
);

CREATE FUNCTION relay_hosted.customer_catalog_listings(origin text,space uuid)
RETURNS SETOF relay_hosted.active_text_listings LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT l.* FROM relay_hosted.active_text_listings l
    JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id AND q.workspace_id=space
    WHERE l.public_origin=origin
$$;

CREATE OR REPLACE FUNCTION relay_hosted.customer_text_models(origin text,provider uuid,binding bytea,token bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('model',l.model,'protocol','responses','revision',l.revision::text,
        'effective_at_s',floor(extract(epoch FROM l.created_at))::bigint,'rates',relay_hosted.text_rates(o)) ORDER BY l.model),'[]')
        FROM relay_hosted.customer_catalog_listings(origin,space) l JOIN relay_hosted.text_offers o USING(id));
END $$;

CREATE FUNCTION relay_hosted.model_status_snapshot(origin text,space uuid,width integer,anchor timestamptz)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    WITH bounds AS (
        SELECT anchor-width*interval '1 hour' AS from_at,
            CASE width WHEN 24 THEN 1800 ELSE 21600 END AS step_s
    ), bins AS (
        SELECT n AS bin,b.from_at+n*b.step_s*interval '1 second' AS lo,
            b.from_at+(n+1)*b.step_s*interval '1 second' AS hi,b.step_s*1000 AS span_ms
        FROM bounds b CROSS JOIN LATERAL generate_series(0,width*3600/b.step_s-1) n
    ), catalog AS MATERIALIZED (
        SELECT l.model,l.item,p.id AS publication,p.revision AS first_revision,p.created_at AS published_at,
            greatest(p.created_at,w.created_at) AS receipt_start
        FROM relay_hosted.customer_catalog_listings(origin,space) l
        JOIN relay_hosted.text_model_rules r ON r.id=l.item
        JOIN relay_hosted.text_offers p ON (p.scope_id,p.source_id,p.model,p.revision)=
            (l.scope_id,l.source_id,l.model,r.first_live_revision) AND p.inventory_id=l.item
        JOIN relay_hosted.workspaces w ON w.id=space
        ORDER BY l.model LIMIT 65
    ), receipts AS MATERIALIZED (
        -- A sentinel row detects overflow; none of a truncated scan is reported.
        SELECT t.id,t.offer_id,t.started_at,t.input_tokens,t.cached_tokens
        FROM relay_hosted.text_requests t,bounds b
        WHERE t.workspace_id=space AND t.started_at>=b.from_at AND t.started_at<anchor
        ORDER BY t.started_at DESC,t.id DESC LIMIT 8193
    ), limits AS (
        SELECT (SELECT count(*) FROM receipts)<=8192 AS complete,(SELECT count(*) FROM catalog)<=64 AS fits
    ), own_usage AS (
        SELECT c.item,floor(extract(epoch FROM r.started_at-b.from_at)/b.step_s)::integer AS bin,
            count(*) AS requests,count(usage.cached) FILTER (WHERE usage.inputs>0) AS cache_samples,
            sum(usage.cached) FILTER (WHERE usage.inputs>0 AND usage.cached IS NOT NULL) AS cached,
            sum(usage.inputs) FILTER (WHERE usage.inputs>0 AND usage.cached IS NOT NULL) AS inputs
        FROM receipts r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        JOIN catalog c ON c.item=o.inventory_id AND o.revision>=c.first_revision AND r.started_at>=c.receipt_start
        LEFT JOIN relay_hosted.text_late_usage late ON late.request_id=r.id
        CROSS JOIN LATERAL (SELECT
            CASE WHEN late.request_id IS NULL THEN r.input_tokens ELSE late.input_tokens END AS inputs,
            CASE WHEN late.request_id IS NULL THEN r.cached_tokens ELSE late.cached_tokens END AS cached) usage
        CROSS JOIN bounds b CROSS JOIN limits f
        WHERE f.complete AND f.fits GROUP BY c.item,bin
    ), cells AS (
        SELECT c.item,b.bin,b.span_ms,h.monitored_ms,h.operational_ms,h.unavailable_ms,
            CASE WHEN f.complete THEN greatest(0,ceil(extract(epoch FROM b.hi-greatest(b.lo,c.receipt_start))*1000))
                ELSE 0 END AS request_ms,
            coalesce(u.requests,0) AS requests,coalesce(u.cache_samples,0) AS cache_samples,u.cached,u.inputs
        FROM catalog c CROSS JOIN bins b CROSS JOIN limits f
        LEFT JOIN own_usage u ON (u.item,u.bin)=(c.item,b.bin)
        CROSS JOIN LATERAL (
            SELECT coalesce(sum(o.ms),0) AS monitored_ms,
                coalesce(sum(o.ms) FILTER (WHERE o.state='operational'),0) AS operational_ms,
                coalesce(sum(o.ms) FILTER (WHERE o.state='unavailable'),0) AS unavailable_ms
            FROM (SELECT s.state,extract(epoch FROM
                least(b.hi,s.slot_at+interval '30 minutes')-greatest(b.lo,s.slot_at))*1000 AS ms
                FROM relay_hosted.model_status_observations s WHERE s.publication_id=c.publication
                AND s.slot_at>b.lo-interval '30 minutes' AND s.slot_at<b.hi
                AND s.slot_at>=c.published_at AND s.slot_at+interval '30 minutes'<=anchor) o
        ) h WHERE f.fits
    ), metrics AS MATERIALIZED (
        SELECT item,bin,jsonb_build_object(
            'state',CASE WHEN coalesce(sum(monitored_ms),0)=0 OR sum(monitored_ms)<sum(span_ms) THEN 'unknown'
                WHEN sum(operational_ms)=sum(span_ms) THEN 'operational'
                WHEN sum(unavailable_ms)=sum(span_ms) THEN 'unavailable' ELSE 'degraded' END,
            'span_ms',coalesce(sum(span_ms),0),'monitored_ms',coalesce(sum(monitored_ms),0)::bigint,
            'operational_ms',coalesce(sum(operational_ms),0)::bigint,
            'request_ms',coalesce(sum(request_ms),0)::bigint,'requests',sum(requests) FILTER (WHERE request_ms>0),
            'latency_ms',NULL,'latency_samples',0,
            'cache_samples',coalesce(sum(cache_samples),0),
            'cache_bps',round(sum(cached)*10000/nullif(sum(inputs),0))) AS value
        FROM cells GROUP BY GROUPING SETS ((item,bin),(item),())
    )
    SELECT CASE WHEN NOT f.fits THEN '{"error":"status_capacity"}'::jsonb ELSE jsonb_build_object(
        'hours',width,'as_of_ms',(extract(epoch FROM anchor)*1000)::bigint,
        'intervals',(SELECT jsonb_agg(jsonb_build_object('from_ms',(extract(epoch FROM b.lo)*1000)::bigint,
            'to_ms',(extract(epoch FROM b.hi)*1000)::bigint) ORDER BY b.bin) FROM bins b),
        'summary',(SELECT m.value FROM metrics m WHERE m.item IS NULL),
        'items',coalesce((SELECT jsonb_agg(jsonb_build_object('model',c.model,
            'metrics',(SELECT m.value FROM metrics m WHERE m.item=c.item AND m.bin IS NULL),
            'history',(SELECT jsonb_agg(m.value ORDER BY m.bin) FROM metrics m WHERE m.item=c.item AND m.bin IS NOT NULL))
            ORDER BY c.model) FROM catalog c),'[]')) END
    FROM limits f
$$;

CREATE FUNCTION relay_hosted.customer_model_status(origin text,provider uuid,binding bytea,token bytea,width integer)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    IF width NOT IN (24,168) THEN RETURN NULL; END IF;
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN relay_hosted.model_status_snapshot(origin,space,width,date_trunc('milliseconds',statement_timestamp()));
END $$;

REVOKE ALL ON TABLE relay_hosted.model_status_observations FROM PUBLIC;
REVOKE ALL ON FUNCTION relay_hosted.customer_catalog_listings(text,uuid),
    relay_hosted.model_status_snapshot(text,uuid,integer,timestamptz),
    relay_hosted.customer_model_status(text,uuid,bytea,bytea,integer) FROM PUBLIC;
