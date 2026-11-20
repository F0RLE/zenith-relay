-- Optional text execution observations, separate from the terminal evidence and journal.
CREATE TABLE relay_hosted.request_diagnostic_kinds (
    code text COLLATE "C" PRIMARY KEY,
    stage text COLLATE "C" NOT NULL,
    origin text COLLATE "C",
    category text COLLATE "C" NOT NULL,
    severity text COLLATE "C" NOT NULL,
    summary_key text NOT NULL,
    cause_status text NOT NULL CHECK (cause_status IN ('confirmed','unknown')),
    needs_permit boolean NOT NULL,
    customer_summary text
);
INSERT INTO relay_hosted.request_diagnostic_kinds VALUES
    ('hosted_dispatch_unavailable','dispatch','relay','unknown','warning','diagnostics.dispatchUnavailable','unknown',false,NULL),
    ('hosted_dispatch_unconfirmed','dispatch','relay','unknown','warning','diagnostics.dispatchUnconfirmed','unknown',false,NULL),
    ('adapter_invalid_request','dispatch','relay','protocol','error','diagnostics.requestPreparation','confirmed',false,NULL),
    ('hosted_execution_deadline','unknown','relay','timeout','warning','diagnostics.executionDeadline','confirmed',false,'interrupted'),
    ('upstream_transport_request','dispatch',NULL,'transport','error','diagnostics.sendUnconfirmed','unknown',true,'interrupted'),
    ('upstream_status','upstream_read',NULL,'unknown','warning','diagnostics.upstreamStatus','unknown',true,'model_unavailable'),
    ('upstream_transport_body','upstream_read',NULL,'transport','error','diagnostics.readInterrupted','unknown',true,'interrupted'),
    ('upstream_transport_timeout','upstream_read',NULL,'timeout','warning','diagnostics.readTimeout','confirmed',true,'interrupted'),
    ('hosted_terminal_missing','upstream_read',NULL,'transport','warning','diagnostics.terminalMissing','confirmed',true,'interrupted'),
    ('hosted_response_limit','upstream_read','relay','policy','warning','diagnostics.responseLimit','confirmed',true,'response_unreadable'),
    ('adapter_upstream_response_invalid','protocol_conversion','relay','protocol','error','diagnostics.responseInvalid','confirmed',true,'response_unreadable'),
    ('adapter_upstream_stream_invalid','protocol_conversion','relay','protocol','error','diagnostics.responseInvalid','confirmed',true,'response_unreadable'),
    ('upstream_terminal','upstream_read','provider','unknown','error','diagnostics.upstreamFailed','unknown',true,'upstream_failed'),
    ('hosted_stream_deadline','upstream_read','relay','timeout','warning','diagnostics.streamDeadline','confirmed',true,'interrupted'),
    ('hosted_delivery_closed','client_delivery',NULL,'cancellation','info','diagnostics.deliveryClosed','confirmed',true,'connection_closed'),
    ('hosted_delivery_backpressure','client_delivery',NULL,'timeout','warning','diagnostics.deliveryBackpressure','confirmed',true,'interrupted'),
    ('hosted_evidence_unavailable','checkpoint','relay','storage','error','diagnostics.evidenceUnavailable','unknown',true,NULL),
    ('hosted_settlement_deferred','settlement','relay','accounting','warning','diagnostics.settlementDeferred','unknown',true,NULL);

CREATE SEQUENCE relay_hosted.request_diagnostic_sequence;
CREATE TABLE relay_hosted.request_diagnostics (
    slot integer PRIMARY KEY CHECK (slot BETWEEN 0 AND 262143),
    sequence bigint NOT NULL,
    event_id uuid NOT NULL DEFAULT gen_random_uuid(),
    request_id uuid NOT NULL,
    attempt_id uuid,
    code text COLLATE "C" NOT NULL REFERENCES relay_hosted.request_diagnostic_kinds,
    http_status smallint CHECK (http_status BETWEEN 100 AND 599 AND code='upstream_status'),
    occurred_ms bigint,
    observed_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    UNIQUE(request_id,code)
);
CREATE INDEX request_diagnostic_expiry ON relay_hosted.request_diagnostics(expires_at,slot);

CREATE FUNCTION relay_hosted.record_request_diagnostic(oid uuid,rid uuid,aid uuid,kind text,status smallint,occurred_ms bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='60ms' AS $$
DECLARE stamp timestamptz:=clock_timestamp(); expiry timestamptz; linked_attempt uuid; started timestamptz; seq bigint;
BEGIN
    IF status IS NOT NULL AND (kind IS DISTINCT FROM 'upstream_status' OR status NOT BETWEEN 100 AND 599) THEN RETURN false; END IF;
    -- Bind to the authorized request and attempt, without taking any request/financial row locks.
    SELECT r.started_at,CASE WHEN k.needs_permit THEN r.attempt_id END
    INTO started,linked_attempt
    FROM relay_hosted.text_requests r JOIN relay_hosted.request_diagnostic_kinds k ON k.code=kind COLLATE "C"
    WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid) AND (NOT k.needs_permit OR r.claimed_at IS NOT NULL);
    IF NOT FOUND THEN RETURN false; END IF;
    expiry:=started+interval '720 hours';
    IF expiry<=stamp THEN RETURN false; END IF;
    IF EXISTS(SELECT FROM relay_hosted.request_diagnostics d WHERE d.request_id=rid AND d.code=kind COLLATE "C")
    THEN RETURN true; END IF;
    seq:=nextval('relay_hosted.request_diagnostic_sequence');
    INSERT INTO relay_hosted.request_diagnostics(slot,sequence,request_id,attempt_id,code,http_status,occurred_ms,observed_at,expires_at)
        VALUES((seq%262144)::integer,seq,rid,linked_attempt,kind,status,
            CASE WHEN occurred_ms BETWEEN floor(extract(epoch FROM started)*1000)
                AND floor(extract(epoch FROM stamp+interval '30 seconds')*1000) THEN occurred_ms END,stamp,expiry)
        ON CONFLICT DO NOTHING;
    RETURN FOUND OR EXISTS(SELECT FROM relay_hosted.request_diagnostics d WHERE d.request_id=rid AND d.code=kind COLLATE "C");
END $$;

CREATE FUNCTION relay_hosted.prune_request_diagnostics()
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH expired AS MATERIALIZED (
        SELECT e.slot FROM relay_hosted.request_diagnostics e
        WHERE e.expires_at<=statement_timestamp() ORDER BY e.expires_at,e.slot LIMIT 1024 FOR UPDATE SKIP LOCKED
    )
    DELETE FROM relay_hosted.request_diagnostics d USING expired e WHERE d.slot=e.slot
$$;

-- One owner projection for structured data. Customer history never uses this type.
CREATE FUNCTION relay_hosted.request_diagnostic_payload(d relay_hosted.request_diagnostics,
    k relay_hosted.request_diagnostic_kinds)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('diagnostic_version',1,'event_id',d.event_id,'sequence',d.sequence::text,
        'request_id',d.request_id,'correlation_id',d.request_id,'attempt_id',d.attempt_id,
        'component','hosted_executor','code',d.code,'stage',k.stage,'origin',k.origin,'category',k.category,
        'severity',k.severity,'summary_key',k.summary_key,'cause_status',k.cause_status,
        'occurred_at_ms',d.occurred_ms,'observed_at_ms',floor(extract(epoch FROM d.observed_at)*1000)::bigint,
        'expires_at_ms',floor(extract(epoch FROM d.expires_at)*1000)::bigint,
        'evidence',jsonb_build_object('http_status',d.http_status))
$$;

ALTER TYPE relay_hosted.owner_request_filter
    ADD ATTRIBUTE stage text, ADD ATTRIBUTE origin text, ADD ATTRIBUTE category text,
    ADD ATTRIBUTE severity text, ADD ATTRIBUTE code text;

CREATE FUNCTION relay_hosted.request_diagnostic_matches(k relay_hosted.request_diagnostic_kinds,f relay_hosted.owner_request_filter)
RETURNS boolean LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
    SELECT (f.stage IS NULL OR k.stage=f.stage COLLATE "C")
        AND (f.origin IS NULL OR k.origin=f.origin COLLATE "C")
        AND (f.category IS NULL OR k.category=f.category COLLATE "C")
        AND (f.severity IS NULL OR k.severity=f.severity COLLATE "C")
        AND (f.code IS NULL OR k.code=f.code COLLATE "C")
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_request_page(origin text,tag bytea,width integer,lo_ms bigint,hi_ms bigint,
    f relay_hosted.owner_request_filter,before_id uuid,errors boolean)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=date_trunc('milliseconds',statement_timestamp()); start_at timestamptz;
    end_at timestamptz; cursor_at timestamptz; now_ms bigint:=(extract(epoch FROM stamp)*1000)::bigint;
    diagnostic_filter boolean:=coalesce(f.stage,f.origin,f.category,f.severity,f.code) IS NOT NULL;
    result jsonb;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF errors IS NULL OR (f.model IS NOT NULL AND f.model COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
        OR f.execution NOT IN ('reserved','dispatched','completed','incomplete','failed','uncertain')
        OR f.accounting NOT IN ('held','pending','settled','released')
        OR f.stage NOT IN ('dispatch','upstream_read','protocol_conversion','client_delivery','checkpoint','settlement','unknown')
        OR f.origin NOT IN ('relay','provider','account')
        OR f.category NOT IN ('policy','timeout','transport','protocol','cancellation','accounting','storage','unknown')
        OR f.severity NOT IN ('info','warning','error','critical')
        OR (f.code IS NOT NULL AND f.code COLLATE "C" !~ '^[a-z][a-z0-9_]{0,63}$') THEN
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
            AND (NOT diagnostic_filter OR EXISTS(
                SELECT FROM relay_hosted.request_diagnostics d JOIN relay_hosted.request_diagnostic_kinds k USING(code)
                WHERE d.request_id=r.id AND d.expires_at>statement_timestamp() AND relay_hosted.request_diagnostic_matches(k,f)))
        ORDER BY r.started_at DESC,r.id DESC LIMIT 33
    ), numbered AS (
        SELECT s.*,row_number() OVER (ORDER BY s.started_at DESC,s.id DESC) AS position FROM selected s
    )
    SELECT jsonb_build_object('version',1,'from_ms',lo_ms,'to_ms',hi_ms,'as_of_ms',now_ms,'errors',errors,
        'items',coalesce(jsonb_agg(jsonb_build_object(
            'request_id',n.id,'started_at_ms',floor(extract(epoch FROM n.started_at)*1000)::bigint,
            'model',n.model,'workspace_id',n.workspace_id,'key_id',n.key_id,'source_id',n.source_id,
            'execution',n.execution,'accounting',n.accounting,'issues',n.codes,
            'posted_micro_usd',CASE WHEN n.closed THEN (n.charged+coalesce(n.credit,0))::text END,
            'diagnostic',(SELECT relay_hosted.request_diagnostic_payload(d,k)
                FROM relay_hosted.request_diagnostics d JOIN relay_hosted.request_diagnostic_kinds k USING(code)
                WHERE d.request_id=n.id AND d.expires_at>statement_timestamp() AND relay_hosted.request_diagnostic_matches(k,f)
                ORDER BY d.sequence DESC LIMIT 1)
        ) ORDER BY n.position) FILTER (WHERE n.position<=32),'[]'),
        'next_before',CASE WHEN count(*)>32 THEN min(n.id::text) FILTER (WHERE n.position=32) END)
    INTO result FROM numbered n;
    RETURN result;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_request_detail(origin text,tag bytea,target uuid)
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
                (7,'credit_posted',j.created_at)) e(position,kind,at) WHERE e.at IS NOT NULL),
        'diagnostics',coalesce(d.events,'[]'),
        'diagnostic_coverage',CASE WHEN d.events IS NULL THEN 'unavailable' ELSE 'partial' END
    ) INTO result FROM relay_hosted.text_requests r
    LEFT JOIN relay_hosted.text_purchase_attempts a ON a.attempt_id=r.attempt_id
    LEFT JOIN relay_hosted.text_inventory i ON i.id=a.item
    LEFT JOIN relay_hosted.text_purchase_results v ON v.attempt_id=r.attempt_id
    LEFT JOIN relay_hosted.text_terminal_evidence t ON t.request_id=r.id
    LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=r.id
    LEFT JOIN relay_hosted.journal_transactions j ON j.request_id=r.id AND j.operation_kind='request_credit'
    LEFT JOIN LATERAL (
        SELECT jsonb_agg(relay_hosted.request_diagnostic_payload(d,k) ORDER BY d.sequence) AS events
        FROM relay_hosted.request_diagnostics d JOIN relay_hosted.request_diagnostic_kinds k USING(code)
        WHERE d.request_id=r.id AND d.expires_at>statement_timestamp()
    ) d ON true WHERE r.id=target;
    RETURN coalesce(result,'{"error":"not_found"}');
END $$;

CREATE FUNCTION relay_hosted.customer_request_diagnostic(r relay_hosted.text_requests)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SET search_path=pg_catalog AS $$
DECLARE summary text; public_code text; measured boolean:=false;
BEGIN
    -- Never project an owner event, even stripped of its secret fields. The
    -- public vocabulary and next action are selected independently.
    IF r.execution='completed' AND r.closed_at IS NULL THEN
        summary:='settling'; public_code:='relay_settlement_pending'; measured:=true;
    ELSIF r.basis='not_sent' THEN
        summary:='not_dispatched'; public_code:='relay_not_dispatched'; measured:=true;
    ELSIF r.execution IN ('incomplete','failed','uncertain') THEN
        SELECT k.customer_summary INTO summary
        FROM relay_hosted.request_diagnostics d JOIN relay_hosted.request_diagnostic_kinds k USING(code)
        WHERE d.request_id=r.id AND d.expires_at>statement_timestamp() AND k.customer_summary IS NOT NULL
            AND (k.customer_summary<>'upstream_failed' OR r.execution='failed')
        ORDER BY (k.customer_summary='upstream_failed') DESC,d.sequence DESC LIMIT 1;
        measured:=FOUND;
        summary:=coalesce(summary,'unavailable');
        public_code:=CASE WHEN summary='upstream_failed' THEN 'relay_upstream_failed'
            WHEN r.closed_at IS NULL THEN 'relay_outcome_pending'
            WHEN r.execution='incomplete' THEN 'relay_request_incomplete'
            WHEN r.execution='failed' THEN 'relay_request_failed' ELSE 'relay_outcome_unknown' END;
    ELSE
        RETURN NULL;
    END IF;
    RETURN jsonb_build_object('diagnostic_version',1,'coverage',CASE WHEN measured THEN 'partial' ELSE 'unavailable' END,
        'code',public_code,'summary',summary,'next_action',CASE WHEN r.closed_at IS NULL THEN 'check_status' ELSE 'contact_support' END);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.read_usage_report(origin text,provider uuid,binding bytea,token bytea,report uuid,
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
        SELECT jsonb_build_object('items',jsonb_build_array(relay_hosted.usage_receipt(r)),
            'diagnostic',relay_hosted.customer_request_diagnostic(r)) INTO result
        FROM relay_hosted.text_requests r WHERE r.workspace_id=space AND r.id=target;
        RETURN coalesce(result,'{"items":[],"diagnostic":null}');
    END IF;
    SELECT relay_hosted.usage_report_payload(h,f,before_id,target,view_kind) INTO result
    FROM relay_hosted.usage_reports h WHERE h.id=report AND h.workspace_id=space AND h.expires_at>clock_timestamp();
    RETURN coalesce(result,'{"error":"report_not_found"}');
END $$;

REVOKE ALL ON FUNCTION relay_hosted.record_request_diagnostic(uuid,uuid,uuid,text,smallint,bigint),
    relay_hosted.prune_request_diagnostics(),
    relay_hosted.request_diagnostic_payload(relay_hosted.request_diagnostics,relay_hosted.request_diagnostic_kinds),
    relay_hosted.request_diagnostic_matches(relay_hosted.request_diagnostic_kinds,relay_hosted.owner_request_filter),
    relay_hosted.customer_request_diagnostic(relay_hosted.text_requests) FROM PUBLIC;
