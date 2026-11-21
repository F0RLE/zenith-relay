CREATE INDEX text_platform_history ON relay_hosted.text_requests(started_at);

-- Project retained request history and the posted inference revenue lines at
-- one snapshot. Group by logical request start time, not wallet posting time.
CREATE FUNCTION relay_hosted.platform_usage_overview(hours integer,model_filter text,anchor timestamptz)
RETURNS jsonb LANGUAGE plpgsql STABLE SET search_path=pg_catalog AS $$
DECLARE start_at timestamptz; step_ms bigint; span_ms bigint; result jsonb;
    empty_bucket jsonb:='{"requests":"0","token_samples":"0","partial_tokens":"0","total_tokens":"0","spend_micro_usd":"0","unsettled":"0"}';
BEGIN
    IF hours=0 THEN
        SELECT date_trunc('milliseconds',min(r.started_at)) INTO start_at
            FROM relay_hosted.text_requests r WHERE r.started_at<anchor;
        start_at:=coalesce(start_at,anchor-interval '24 hours');
    ELSE
        start_at:=anchor-hours*interval '1 hour';
    END IF;
    span_ms:=(extract(epoch FROM anchor-start_at)*1000)::bigint;
    step_ms:=CASE hours WHEN 0 THEN (span_ms+29)/30 WHEN 24 THEN 3600000 ELSE 86400000 END;

    WITH measured AS (
        SELECT floor(extract(epoch FROM r.started_at-start_at)*1000/step_ms)::integer AS bucket,
            o.model,model_filter IS NULL OR o.model=model_filter COLLATE "C" AS picked,
            coalesce(l.input_tokens::numeric+l.output_tokens,r.input_tokens::numeric+r.output_tokens) AS tokens,
            l.request_id IS NULL AND r.basis IS DISTINCT FROM 'provider_final'
                AND (r.input_tokens IS NOT NULL OR r.output_tokens IS NOT NULL) AS partial,
            r.closed_at IS NULL AS unsettled,
            coalesce(debit.amount_micro_usd,0)::numeric+coalesce(credit.amount_micro_usd,0) AS spend
        FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=r.id
        LEFT JOIN relay_hosted.journal_transactions j ON j.request_id=r.id AND j.operation_kind='request_close'
        LEFT JOIN relay_hosted.journal_postings debit ON debit.transaction_id=j.id AND debit.sequence=3
        LEFT JOIN relay_hosted.journal_transactions c ON c.request_id=r.id AND c.operation_kind='request_credit'
        LEFT JOIN relay_hosted.journal_postings credit ON credit.transaction_id=c.id AND credit.sequence=3
        WHERE r.started_at>=start_at AND r.started_at<anchor
    ), grouped AS (
        SELECT bucket,model,count(*) FILTER (WHERE picked) AS requests,
            count(tokens) FILTER (WHERE picked) AS token_samples,
            count(*) FILTER (WHERE picked AND partial) AS partial_tokens,
            sum(tokens) FILTER (WHERE picked) AS tokens,
            coalesce(sum(spend) FILTER (WHERE picked),0) AS spend,
            count(*) FILTER (WHERE picked AND unsettled) AS unsettled
        FROM measured GROUP BY GROUPING SETS ((),(bucket),(model))
    ), payloads AS (
        SELECT jsonb_object_agg(coalesce(bucket,-1),jsonb_build_object(
            'requests',requests::text,'token_samples',token_samples::text,'partial_tokens',partial_tokens::text,
            'total_tokens',CASE WHEN requests=0 THEN '0' ELSE tokens::text END,
            'spend_micro_usd',spend::text,'unsettled',unsettled::text)) AS stats
        FROM grouped WHERE model IS NULL
    ), choices AS (
        SELECT coalesce(jsonb_agg(model ORDER BY position) FILTER (WHERE position<=256),'[]') AS models,
            count(*)>256 AS more
        FROM (SELECT model,row_number() OVER (ORDER BY model) AS position
            FROM grouped WHERE model IS NOT NULL ORDER BY model LIMIT 257) m
    )
    SELECT jsonb_build_object('hours',hours,'model',model_filter,'currency','USD','precision',6,
        'from_ms',(extract(epoch FROM start_at)*1000)::bigint,'as_of_ms',(extract(epoch FROM anchor)*1000)::bigint,
        'step_ms',step_ms,'totals',p.stats->'-1','models',c.models,'more_models',c.more,
        'buckets',(SELECT jsonb_agg(coalesce(p.stats->i::text,empty_bucket) ORDER BY i)
            FROM generate_series(0,(span_ms-1)/step_ms) i))
    INTO result FROM payloads p CROSS JOIN choices c;
    RETURN result;
END $$;

CREATE FUNCTION relay_hosted.owner_usage_overview(origin text,tag bytea,hours integer,model_filter text)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF NOT EXISTS (
        SELECT FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.admin_sessions s
        ON (s.user_id,s.credential_id,s.installation_epoch,s.user_epoch)=
           (a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
        WHERE a.public_origin=origin AND s.verifier=tag AND s.expires_at>statement_timestamp()
            AND s.last_seen_at>statement_timestamp()-interval '20 minutes'
    ) THEN RETURN NULL; END IF;
    IF hours IS NULL OR hours NOT IN (0,24,168,720)
        OR (model_filter IS NOT NULL AND model_filter COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
    THEN RETURN '{"error":"invalid_filter"}'; END IF;
    RETURN relay_hosted.platform_usage_overview(hours,model_filter,date_trunc('milliseconds',statement_timestamp()));
END $$;

REVOKE ALL ON FUNCTION relay_hosted.platform_usage_overview(integer,text,timestamptz),
    relay_hosted.owner_usage_overview(text,bytea,integer,text) FROM PUBLIC;
