CREATE TYPE relay_hosted.capture_reason AS ENUM ('request_timing','stream_interruption','pending_outcome');
CREATE TYPE relay_hosted.capture_stop AS ENUM ('stopped','expired','exhausted','revoked');
CREATE TYPE relay_hosted.capture_milestone AS ENUM (
    'dispatch_committed','upstream_response','first_stream_frame','terminal_observed',
    'evidence_committed','accounting_settled','accounting_pending','request_interrupted'
);

CREATE TABLE relay_hosted.diagnostic_sessions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    operation_id uuid NOT NULL UNIQUE,
    request_id uuid NOT NULL,
    origin text NOT NULL,
    proof_tag bytea CHECK (octet_length(proof_tag)=32),
    reason relay_hosted.capture_reason NOT NULL,
    started_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at>started_at AND expires_at<=started_at+interval '30 minutes'),
    ended_at timestamptz CHECK (ended_at BETWEEN started_at AND expires_at),
    stop_reason relay_hosted.capture_stop,
    stopped_by uuid REFERENCES relay_hosted.users,
    closure_audited boolean NOT NULL DEFAULT false CHECK (NOT closure_audited OR ended_at IS NOT NULL),
    max_events integer NOT NULL CHECK (max_events BETWEEN 1 AND 128),
    max_bytes integer NOT NULL CHECK (max_bytes BETWEEN 128 AND 32768),
    max_rate integer NOT NULL CHECK (max_rate BETWEEN 1 AND 16),
    consumed_events integer NOT NULL DEFAULT 0 CHECK (consumed_events BETWEEN 0 AND max_events),
    consumed_bytes integer NOT NULL DEFAULT 0 CHECK (consumed_bytes BETWEEN 0 AND max_bytes),
    CHECK ((ended_at IS NULL)=(stop_reason IS NULL) AND (ended_at IS NULL)=(proof_tag IS NOT NULL)),
    CHECK ((stopped_by IS NOT NULL)=coalesce(stop_reason='stopped',false))
);
CREATE UNIQUE INDEX one_active_diagnostic_session ON relay_hosted.diagnostic_sessions ((true)) WHERE ended_at IS NULL;
CREATE INDEX diagnostic_session_history ON relay_hosted.diagnostic_sessions (started_at DESC,id DESC);

CREATE TABLE relay_hosted.diagnostic_capture_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    session_id uuid REFERENCES relay_hosted.diagnostic_sessions ON DELETE SET NULL,
    request_id uuid NOT NULL,
    sequence integer NOT NULL CHECK (sequence BETWEEN 1 AND 128),
    code relay_hosted.capture_milestone NOT NULL,
    occurred_ms bigint NOT NULL,
    recorded_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at>recorded_at),
    UNIQUE(session_id,sequence),
    UNIQUE(session_id,code)
);
CREATE INDEX diagnostic_capture_event_expiry ON relay_hosted.diagnostic_capture_events (expires_at,id);

-- Independent IDs survive metadata pruning; only the recovery janitor can publish a close record.
CREATE TABLE relay_hosted.diagnostic_capture_audit (
    session_id uuid NOT NULL,
    action text NOT NULL CHECK (action IN ('start','close')),
    operation_id uuid UNIQUE,
    actor_id uuid,
    at timestamptz NOT NULL,
    reason relay_hosted.capture_reason,
    stop_reason relay_hosted.capture_stop,
    PRIMARY KEY(session_id,action),
    CHECK ((action='start')=(operation_id IS NOT NULL) AND (action='start')=(reason IS NOT NULL)
        AND (action='close')=(stop_reason IS NOT NULL)),
    CHECK ((actor_id IS NOT NULL)=(action='start' OR coalesce(stop_reason='stopped',false)))
);
CREATE INDEX diagnostic_capture_audit_expiry ON relay_hosted.diagnostic_capture_audit (at);

CREATE FUNCTION relay_hosted.lock_capture_owner(p_origin text,p_tag bytea)
RETURNS relay_hosted.admin_sessions LANGUAGE sql SET search_path=pg_catalog AS $$
    SELECT s.* FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.admin_sessions s
    ON (s.user_id,s.credential_id,s.installation_epoch,s.user_epoch)=
       (a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
    WHERE a.public_origin=p_origin AND s.verifier=p_tag AND s.expires_at>clock_timestamp()
        AND s.last_seen_at>clock_timestamp()-interval '20 minutes'
    FOR SHARE OF s,a NOWAIT
$$;

CREATE FUNCTION relay_hosted.capture_stop_condition(d relay_hosted.diagnostic_sessions,stamp timestamptz)
RETURNS relay_hosted.capture_stop LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT CASE
        WHEN d.ended_at IS NOT NULL THEN d.stop_reason
        WHEN stamp>=d.expires_at THEN 'expired'::relay_hosted.capture_stop
        WHEN stamp<d.started_at OR relay_hosted.owner_usage_session(d.origin,d.proof_tag) IS NOT TRUE THEN 'revoked'
    END
$$;

CREATE FUNCTION relay_hosted.close_diagnostic_capture(
    sid uuid,cause relay_hosted.capture_stop,stamp timestamptz,actor uuid DEFAULT NULL
) RETURNS void LANGUAGE sql SET search_path=pg_catalog AS $$
    UPDATE relay_hosted.diagnostic_sessions SET proof_tag=NULL,stop_reason=cause,stopped_by=actor,
        ended_at=CASE WHEN cause='expired' THEN expires_at ELSE least(expires_at,greatest(started_at,stamp)) END
    WHERE id=sid AND ended_at IS NULL
$$;

CREATE FUNCTION relay_hosted.capture_millis(stamp timestamptz)
RETURNS bigint LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT floor(extract(epoch FROM stamp)*1000)::bigint
$$;

CREATE FUNCTION relay_hosted.capture_summary(d relay_hosted.diagnostic_sessions,stamp timestamptz)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',d.id,'operation_id',d.operation_id,'request_id',d.request_id,'reason',d.reason,
        'state',coalesce(cause::text,'active'),
        'started_at_ms',relay_hosted.capture_millis(d.started_at),'expires_at_ms',relay_hosted.capture_millis(d.expires_at),
        'ended_at_ms',relay_hosted.capture_millis(coalesce(d.ended_at,CASE WHEN stamp>=d.expires_at THEN d.expires_at END)),
        'limits',jsonb_build_object('events',d.max_events,'bytes',d.max_bytes,'events_per_second',d.max_rate),
        'consumed',jsonb_build_object('events',d.consumed_events,'bytes',d.consumed_bytes),
        'audit_pending',cause IS NOT NULL AND NOT d.closure_audited)
    FROM relay_hosted.capture_stop_condition(d,stamp) AS cause
$$;

CREATE FUNCTION relay_hosted.owner_diagnostic_captures(p_origin text,p_tag bytea,p_id uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=statement_timestamp(); d relay_hosted.diagnostic_sessions;
BEGIN
    IF relay_hosted.owner_usage_session(p_origin,p_tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF p_id IS NULL THEN
        RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),'items',coalesce((
            SELECT jsonb_agg(relay_hosted.capture_summary(s,stamp) ORDER BY (s.ended_at IS NULL) DESC,s.started_at DESC,s.id DESC)
            FROM (SELECT * FROM relay_hosted.diagnostic_sessions
                WHERE origin=p_origin AND coalesce(ended_at,least(stamp,expires_at))>stamp-interval '336 hours'
                ORDER BY (ended_at IS NULL) DESC,started_at DESC,id DESC LIMIT 12) s
        ),'[]'));
    END IF;
    SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE id=p_id AND origin=p_origin
        AND coalesce(ended_at,least(stamp,expires_at))>stamp-interval '336 hours';
    IF NOT FOUND THEN RETURN '{"error":"not_found"}'; END IF;
    RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),
        'session',relay_hosted.capture_summary(d,stamp),'coverage','partial',
        'events',coalesce((SELECT jsonb_agg(jsonb_build_object(
            'sequence',e.sequence,'code',e.code,'occurred_at_ms',e.occurred_ms) ORDER BY e.sequence)
            FROM relay_hosted.diagnostic_capture_events e WHERE e.session_id=d.id AND e.expires_at>stamp),'[]'));
END $$;

CREATE FUNCTION relay_hosted.start_diagnostic_capture(
    p_origin text,p_tag bytea,p_operation uuid,p_request uuid,p_reason relay_hosted.capture_reason,
    seconds integer,max_seconds integer,event_limit integer,byte_limit integer,rate_limit integer
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='150ms' AS $$
DECLARE proof relay_hosted.admin_sessions; d relay_hosted.diagnostic_sessions; prior uuid;
    stamp timestamptz; cause relay_hosted.capture_stop;
BEGIN
    IF relay_hosted.owner_usage_session(p_origin,p_tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF p_operation IS NULL OR p_operation='00000000-0000-0000-0000-000000000000'
        OR p_request IS NULL OR p_reason IS NULL OR seconds IS NULL OR seconds NOT BETWEEN 1 AND 1800
    THEN RETURN '{"error":"invalid_capture"}'; END IF;
    -- Starts serialize only with other starts. Disarming uses the individual capture row.
    PERFORM pg_advisory_xact_lock(65082942);
    SELECT session_id INTO prior FROM relay_hosted.diagnostic_capture_audit WHERE operation_id=p_operation;
    IF FOUND THEN
        SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE id=prior;
        stamp:=clock_timestamp();
        IF NOT FOUND OR coalesce(d.ended_at,d.expires_at)<=stamp-interval '336 hours'
        THEN RETURN '{"error":"capture_retired"}'; END IF;
        IF d.origin<>p_origin OR d.request_id<>p_request OR d.reason<>p_reason
            OR d.expires_at-d.started_at<>make_interval(secs=>seconds)
        THEN RETURN '{"error":"capture_conflict"}'; END IF;
        RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),
            'session',relay_hosted.capture_summary(d,stamp));
    END IF;
    IF max_seconds IS NULL OR event_limit IS NULL OR byte_limit IS NULL OR rate_limit IS NULL
    THEN RETURN '{"error":"capture_disabled"}'; END IF;
    IF max_seconds NOT BETWEEN 1 AND 1800 OR seconds>max_seconds OR event_limit NOT BETWEEN 1 AND 128
        OR byte_limit NOT BETWEEN 128 AND 32768 OR rate_limit NOT BETWEEN 1 AND 16
    THEN RETURN '{"error":"invalid_capture"}'; END IF;

    SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE ended_at IS NULL FOR UPDATE;
    stamp:=clock_timestamp();
    IF FOUND THEN
        cause:=relay_hosted.capture_stop_condition(d,stamp);
        IF cause IS NULL THEN RETURN '{"error":"capture_busy"}'; END IF;
        PERFORM relay_hosted.close_diagnostic_capture(d.id,cause,stamp);
    END IF;
    proof:=relay_hosted.lock_capture_owner(p_origin,p_tag);
    stamp:=clock_timestamp();
    IF proof.verifier IS NULL OR proof.expires_at<=stamp OR proof.last_seen_at<=stamp-interval '20 minutes'
    THEN RETURN NULL; END IF;
    IF proof.created_at<=stamp-interval '2 minutes' OR proof.created_at>stamp
    THEN RETURN '{"error":"fresh_passkey_required"}'; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.text_requests r WHERE r.id=p_request
        AND r.started_at<=stamp AND r.close_by>stamp AND r.closed_at IS NULL
        AND r.execution IN ('reserved','dispatched'))
    THEN RETURN '{"error":"capture_request_unavailable"}'; END IF;
    IF (SELECT count(*) FROM relay_hosted.diagnostic_capture_audit WHERE action='start')>=1024
    THEN RETURN '{"error":"capture_history_full"}'; END IF;
    INSERT INTO relay_hosted.diagnostic_sessions(operation_id,request_id,origin,proof_tag,reason,started_at,expires_at,
        max_events,max_bytes,max_rate)
    VALUES(p_operation,p_request,p_origin,p_tag,p_reason,stamp,stamp+make_interval(secs=>seconds),
        event_limit,byte_limit,rate_limit) RETURNING * INTO d;
    INSERT INTO relay_hosted.diagnostic_capture_audit(session_id,action,operation_id,actor_id,at,reason)
        VALUES(d.id,'start',p_operation,proof.user_id,stamp,p_reason);
    RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),
        'session',relay_hosted.capture_summary(d,stamp));
END $$;

CREATE FUNCTION relay_hosted.stop_diagnostic_capture(p_origin text,p_tag bytea,p_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='150ms' AS $$
DECLARE proof relay_hosted.admin_sessions; d relay_hosted.diagnostic_sessions;
    stamp timestamptz; cause relay_hosted.capture_stop;
BEGIN
    IF relay_hosted.owner_usage_session(p_origin,p_tag) IS NOT TRUE THEN RETURN NULL; END IF;
    SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE id=p_id AND origin=p_origin FOR UPDATE;
    IF NOT FOUND THEN RETURN '{"error":"not_found"}'; END IF;
    proof:=relay_hosted.lock_capture_owner(p_origin,p_tag);
    stamp:=clock_timestamp();
    IF proof.verifier IS NULL OR proof.expires_at<=stamp OR proof.last_seen_at<=stamp-interval '20 minutes'
    THEN RETURN NULL; END IF;
    IF coalesce(d.ended_at,d.expires_at)<=stamp-interval '336 hours' THEN RETURN '{"error":"not_found"}'; END IF;
    IF d.ended_at IS NULL THEN
        cause:=coalesce(relay_hosted.capture_stop_condition(d,stamp),'stopped');
        PERFORM relay_hosted.close_diagnostic_capture(d.id,cause,stamp,CASE WHEN cause='stopped' THEN proof.user_id END);
        SELECT * INTO STRICT d FROM relay_hosted.diagnostic_sessions WHERE id=p_id;
    END IF;
    RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),
        'session',relay_hosted.capture_summary(d,stamp));
END $$;

-- The ingress learns only the bound target and fence. This read cannot start or prolong capture.
CREATE FUNCTION relay_hosted.capture_target(p_origin text,max_seconds integer,event_limit integer,byte_limit integer,rate_limit integer)
RETURNS TABLE(session_id uuid,request_id uuid) LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT d.id,d.request_id FROM relay_hosted.diagnostic_sessions d
    WHERE d.origin=p_origin AND d.ended_at IS NULL AND relay_hosted.capture_stop_condition(d,statement_timestamp()) IS NULL
        AND d.expires_at-d.started_at<=make_interval(secs=>max_seconds)
        AND d.max_events<=event_limit AND d.max_bytes<=byte_limit AND d.max_rate<=rate_limit
$$;

CREATE FUNCTION relay_hosted.append_capture_milestone(
    sid uuid,rid uuid,aid uuid,milestone relay_hosted.capture_milestone,observed_ms bigint
) RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='60ms' AS $$
DECLARE d relay_hosted.diagnostic_sessions; proof relay_hosted.admin_sessions; stamp timestamptz;
    expiry timestamptz; seq integer; size integer; cause relay_hosted.capture_stop;
BEGIN
    SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE id=sid AND request_id=rid AND ended_at IS NULL FOR UPDATE;
    IF NOT FOUND THEN RETURN true; END IF;
    stamp:=clock_timestamp();
    cause:=relay_hosted.capture_stop_condition(d,stamp);
    IF cause IS NULL THEN
        proof:=relay_hosted.lock_capture_owner(d.origin,d.proof_tag);
        stamp:=clock_timestamp();
        IF stamp>=d.expires_at THEN cause:='expired';
        ELSIF proof.verifier IS NULL OR proof.expires_at<=stamp OR proof.last_seen_at<=stamp-interval '20 minutes'
            OR stamp<d.started_at THEN cause:='revoked';
        END IF;
    END IF;
    IF cause IS NOT NULL THEN
        PERFORM relay_hosted.close_diagnostic_capture(sid,cause,stamp);
        RETURN true;
    END IF;
    IF observed_ms<relay_hosted.capture_millis(d.started_at) OR observed_ms>=relay_hosted.capture_millis(d.expires_at)
        OR observed_ms>relay_hosted.capture_millis(stamp)
    THEN RETURN false; END IF;
    SELECT r.started_at+interval '720 hours' INTO expiry FROM relay_hosted.text_requests r
    WHERE (r.id,r.attempt_id)=(rid,aid) AND (milestone<>'dispatch_committed' OR r.claimed_at IS NOT NULL);
    IF NOT FOUND OR expiry<=stamp THEN RETURN false; END IF;
    IF EXISTS(SELECT FROM relay_hosted.diagnostic_capture_events WHERE session_id=sid AND code=milestone)
    THEN RETURN true; END IF;
    seq:=d.consumed_events+1;
    size:=octet_length(jsonb_build_object('sequence',seq,'code',milestone,'occurred_at_ms',observed_ms)::text)+1;
    IF seq>d.max_events OR d.consumed_bytes+size>d.max_bytes OR (
        SELECT count(*) FROM relay_hosted.diagnostic_capture_events WHERE session_id=sid
            AND recorded_at>stamp-interval '1 second'
    )>=d.max_rate THEN
        PERFORM relay_hosted.close_diagnostic_capture(sid,'exhausted',stamp);
        RETURN true;
    END IF;
    INSERT INTO relay_hosted.diagnostic_capture_events(session_id,request_id,sequence,code,occurred_ms,recorded_at,expires_at)
        VALUES(sid,rid,seq,milestone,observed_ms,stamp,expiry);
    UPDATE relay_hosted.diagnostic_sessions SET consumed_events=seq,consumed_bytes=d.consumed_bytes+size WHERE id=sid;
    IF seq=d.max_events OR d.consumed_bytes+size=d.max_bytes THEN
        PERFORM relay_hosted.close_diagnostic_capture(sid,'exhausted',stamp);
    END IF;
    RETURN true;
END $$;

-- Commit automatic disarm before trying the audit sink. Closed state is also the durable recovery record.
CREATE FUNCTION relay_hosted.sweep_diagnostic_capture()
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='100ms' AS $$
DECLARE d relay_hosted.diagnostic_sessions; stamp timestamptz; cause relay_hosted.capture_stop;
BEGIN
    SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE ended_at IS NULL FOR UPDATE SKIP LOCKED;
    IF FOUND THEN
        stamp:=clock_timestamp();
        cause:=relay_hosted.capture_stop_condition(d,stamp);
        IF cause IS NOT NULL THEN PERFORM relay_hosted.close_diagnostic_capture(d.id,cause,stamp); END IF;
    END IF;
END $$;

CREATE FUNCTION relay_hosted.prune_diagnostic_captures()
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='100ms' AS $$
DECLARE stamp timestamptz:=clock_timestamp();
BEGIN
    WITH closed AS (
        SELECT id,stopped_by,ended_at,stop_reason FROM relay_hosted.diagnostic_sessions
        WHERE ended_at IS NOT NULL AND NOT closure_audited
        ORDER BY ended_at,id LIMIT 64 FOR UPDATE SKIP LOCKED
    ), audited AS (
        INSERT INTO relay_hosted.diagnostic_capture_audit(session_id,action,actor_id,at,stop_reason)
        SELECT id,'close',stopped_by,ended_at,stop_reason FROM closed RETURNING session_id
    )
    UPDATE relay_hosted.diagnostic_sessions d SET closure_audited=true FROM audited a WHERE d.id=a.session_id;
    DELETE FROM relay_hosted.diagnostic_sessions WHERE ended_at<=stamp-interval '336 hours' AND closure_audited;
    DELETE FROM relay_hosted.diagnostic_capture_audit a WHERE a.at<=stamp-interval '4320 hours'
        AND NOT EXISTS(SELECT FROM relay_hosted.diagnostic_sessions d WHERE d.id=a.session_id);
END $$;

CREATE FUNCTION relay_hosted.prune_capture_events()
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='100ms' AS $$
    DELETE FROM relay_hosted.diagnostic_capture_events WHERE id IN (
        SELECT id FROM relay_hosted.diagnostic_capture_events WHERE expires_at<=statement_timestamp()
        ORDER BY expires_at,id LIMIT 1024 FOR UPDATE SKIP LOCKED
    )
$$;

-- `serve` has a process-local cookie seal. Discard proofs the new process cannot verify.
CREATE FUNCTION relay_hosted.reset_owner_browser_sessions(p_origin text)
RETURNS void LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    DELETE FROM relay_hosted.admin_sessions s USING relay_hosted.installations i
    WHERE i.singleton AND i.public_origin=p_origin AND s.user_id=i.owner_user_id
$$;

REVOKE EXECUTE ON FUNCTION relay_hosted.lock_capture_owner(text,bytea),
    relay_hosted.capture_stop_condition(relay_hosted.diagnostic_sessions,timestamptz),
    relay_hosted.close_diagnostic_capture(uuid,relay_hosted.capture_stop,timestamptz,uuid),
    relay_hosted.capture_millis(timestamptz),relay_hosted.capture_summary(relay_hosted.diagnostic_sessions,timestamptz),
    relay_hosted.owner_diagnostic_captures(text,bytea,uuid),
    relay_hosted.start_diagnostic_capture(text,bytea,uuid,uuid,relay_hosted.capture_reason,integer,integer,integer,integer,integer),
    relay_hosted.stop_diagnostic_capture(text,bytea,uuid),relay_hosted.capture_target(text,integer,integer,integer,integer),
    relay_hosted.append_capture_milestone(uuid,uuid,uuid,relay_hosted.capture_milestone,bigint),
    relay_hosted.sweep_diagnostic_capture(),relay_hosted.prune_diagnostic_captures(),relay_hosted.prune_capture_events(),
    relay_hosted.reset_owner_browser_sessions(text) FROM PUBLIC;
