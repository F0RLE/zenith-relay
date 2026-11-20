CREATE INDEX text_platform_request_order ON relay_hosted.text_requests(started_at DESC,id DESC);
DROP INDEX relay_hosted.text_platform_history;

CREATE FUNCTION relay_hosted.owner_usage_session(origin text,tag bytea)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS (
        SELECT FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.admin_sessions s
        ON (s.user_id,s.credential_id,s.installation_epoch,s.user_epoch)=
           (a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
        WHERE a.public_origin=origin AND s.verifier=tag AND s.expires_at>statement_timestamp()
            AND s.last_seen_at>statement_timestamp()-interval '20 minutes'
    )
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_usage_overview(origin text,tag bytea,hours integer,model_filter text)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF hours IS NULL OR hours NOT IN (0,24,168,720)
        OR (model_filter IS NOT NULL AND model_filter COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
    THEN RETURN '{"error":"invalid_filter"}'; END IF;
    RETURN relay_hosted.platform_usage_overview(hours,model_filter,date_trunc('milliseconds',statement_timestamp()));
END $$;

CREATE TYPE relay_hosted.owner_request_filter AS (
    model text, workspace uuid, key_id uuid, source_id uuid, execution text, accounting text
);

-- These describe retained facts, not an inferred provider/account fault.
CREATE FUNCTION relay_hosted.owner_request_issues(execution text,basis text,closed boolean,late boolean,credit_pending boolean)
RETURNS text[] LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
    SELECT array_remove(ARRAY[
        CASE WHEN execution IN ('incomplete','failed','uncertain') THEN 'execution' END,
        CASE WHEN execution='completed' AND NOT closed THEN 'settlement' END,
        CASE WHEN basis IN ('provider_partial','policy_waiver') AND NOT late THEN 'usage_evidence' END,
        CASE WHEN credit_pending THEN 'credit_pending' END
    ],NULL)
$$;

CREATE FUNCTION relay_hosted.owner_request_page(origin text,tag bytea,width integer,lo_ms bigint,hi_ms bigint,
    f relay_hosted.owner_request_filter,before_id uuid,errors boolean)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=date_trunc('milliseconds',statement_timestamp()); start_at timestamptz;
    end_at timestamptz; cursor_at timestamptz; now_ms bigint:=(extract(epoch FROM stamp)*1000)::bigint;
    result jsonb;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF errors IS NULL OR (f.model IS NOT NULL AND f.model COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
        OR f.execution NOT IN ('reserved','dispatched','completed','incomplete','failed','uncertain')
        OR f.accounting NOT IN ('held','pending','settled','released') THEN
        RETURN '{"error":"invalid_filter"}';
    END IF;
    IF lo_ms IS NULL AND hi_ms IS NULL AND before_id IS NULL AND coalesce(width,168) IN (24,168,720) THEN
        lo_ms:=now_ms-coalesce(width,168)*3600000::bigint;
        hi_ms:=now_ms;
    ELSIF width IS NOT NULL OR lo_ms IS NULL OR hi_ms IS NULL OR NOT (0<lo_ms AND lo_ms<hi_ms AND hi_ms<=now_ms) THEN
        RETURN '{"error":"invalid_filter"}';
    END IF;
    start_at:=timestamptz '1970-01-01 00:00:00+00'+(lo_ms/1000)*interval '1 second'+(lo_ms%1000)*interval '1 millisecond';
    end_at:=timestamptz '1970-01-01 00:00:00+00'+(hi_ms/1000)*interval '1 second'+(hi_ms%1000)*interval '1 millisecond';
    IF before_id IS NOT NULL THEN
        SELECT r.started_at INTO cursor_at FROM relay_hosted.text_requests r
            WHERE r.id=before_id AND r.started_at>=start_at AND r.started_at<end_at;
        IF NOT FOUND THEN RETURN '{"error":"cursor_unavailable"}'; END IF;
    END IF;
    WITH selected AS MATERIALIZED (
        SELECT r.id,r.started_at,o.model,r.workspace_id,r.key_id,i.source_id,r.execution,r.accounting,
            c.amount_micro_usd AS credit, debit.id IS NOT NULL AS closed,r.charged,issue.codes
        FROM relay_hosted.text_requests r
        JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        LEFT JOIN relay_hosted.text_purchase_attempts a ON a.attempt_id=r.attempt_id
        LEFT JOIN relay_hosted.text_inventory i ON i.id=a.item
        LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=r.id
        LEFT JOIN relay_hosted.journal_transactions debit ON debit.request_id=r.id AND debit.operation_kind='request_close'
        LEFT JOIN relay_hosted.journal_transactions j ON j.request_id=r.id AND j.operation_kind='request_credit'
        LEFT JOIN relay_hosted.journal_postings c ON c.transaction_id=j.id AND c.sequence=3
        CROSS JOIN LATERAL (SELECT relay_hosted.owner_request_issues(r.execution,r.basis,r.closed_at IS NOT NULL,
            l.request_id IS NOT NULL,r.basis='provider_partial' AND r.charged>l.valued_micro_usd AND j.id IS NULL) AS codes) issue
        WHERE r.started_at>=start_at AND r.started_at<end_at
            AND (cursor_at IS NULL OR (r.started_at,r.id)<(cursor_at,before_id))
            AND (f.model IS NULL OR o.model=f.model COLLATE "C")
            AND (f.workspace IS NULL OR r.workspace_id=f.workspace)
            AND (f.key_id IS NULL OR r.key_id=f.key_id)
            AND (f.source_id IS NULL OR i.source_id=f.source_id)
            AND (f.execution IS NULL OR r.execution=f.execution)
            AND (f.accounting IS NULL OR r.accounting=f.accounting)
            AND (NOT errors OR cardinality(issue.codes)>0)
        ORDER BY r.started_at DESC,r.id DESC LIMIT 33
    ), numbered AS (
        SELECT s.*,row_number() OVER (ORDER BY s.started_at DESC,s.id DESC) AS position FROM selected s
    )
    SELECT jsonb_build_object('version',1,'from_ms',lo_ms,'to_ms',hi_ms,'as_of_ms',now_ms,'errors',errors,
        'items',coalesce(jsonb_agg(jsonb_build_object(
            'request_id',n.id,'started_at_ms',floor(extract(epoch FROM n.started_at)*1000)::bigint,
            'model',n.model,'workspace_id',n.workspace_id,'key_id',n.key_id,'source_id',n.source_id,
            'execution',n.execution,'accounting',n.accounting,'issues',n.codes,
            'posted_micro_usd',CASE WHEN n.closed THEN (n.charged+coalesce(n.credit,0))::text END
        ) ORDER BY n.position) FILTER (WHERE n.position<=32),'[]'),
        'next_before',CASE WHEN count(*)>32 THEN min(n.id::text) FILTER (WHERE n.position=32) END)
    INTO result FROM numbered n;
    RETURN result;
END $$;

CREATE FUNCTION relay_hosted.owner_request_detail(origin text,tag bytea,target uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE result jsonb;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    SELECT jsonb_build_object(
        'version',1,'as_of_ms',floor(extract(epoch FROM statement_timestamp())*1000)::bigint,
        'receipt',relay_hosted.usage_receipt(r),'workspace_id',r.workspace_id,'streaming',r.streaming,
        'issues',relay_hosted.owner_request_issues(r.execution,r.basis,r.closed_at IS NOT NULL,
            l.request_id IS NOT NULL,r.basis='provider_partial' AND r.charged>l.valued_micro_usd AND j.id IS NULL),
        'attempt',CASE WHEN r.claimed_at IS NOT NULL THEN jsonb_build_object(
            'id',r.attempt_id,'source_id',i.source_id,'purchase_revision',a.rate_revision::text,
            'valued_micro_usd',v.valued_micro_usd::text,'terminal',t.outcome) END,
        'timeline',(SELECT jsonb_agg(jsonb_build_object('event',e.kind,
            'at_ms',floor(extract(epoch FROM e.at)*1000)::bigint) ORDER BY e.at,e.position)
            FROM (VALUES (1,'reserved',r.started_at),(2,'dispatch_permitted',r.claimed_at),
                (3,'terminal_observed',t.observed_at),(4,'terminal_applied',t.applied_at),
                (5,'hold_closed',r.closed_at),(6,'late_usage',l.observed_at),
                (7,'credit_posted',j.created_at)) e(position,kind,at) WHERE e.at IS NOT NULL)
    ) INTO result FROM relay_hosted.text_requests r
    LEFT JOIN relay_hosted.text_purchase_attempts a ON a.attempt_id=r.attempt_id
    LEFT JOIN relay_hosted.text_inventory i ON i.id=a.item
    LEFT JOIN relay_hosted.text_purchase_results v ON v.attempt_id=r.attempt_id
    LEFT JOIN relay_hosted.text_terminal_evidence t ON t.request_id=r.id
    LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=r.id
    LEFT JOIN relay_hosted.journal_transactions j ON j.request_id=r.id AND j.operation_kind='request_credit'
    WHERE r.id=target;
    RETURN coalesce(result,'{"error":"not_found"}');
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_usage_session(text,bytea),
    relay_hosted.owner_request_issues(text,text,boolean,boolean,boolean),
    relay_hosted.owner_request_page(text,bytea,integer,bigint,bigint,relay_hosted.owner_request_filter,uuid,boolean),
    relay_hosted.owner_request_detail(text,bytea,uuid) FROM PUBLIC;
