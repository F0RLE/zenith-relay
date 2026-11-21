CREATE TABLE relay_hosted.operation_jobs (
    job text PRIMARY KEY,
    stale_after_seconds integer NOT NULL CHECK (stale_after_seconds > 0),
    observed_at timestamptz,
    succeeded boolean,
    history_gap_at timestamptz,
    CHECK ((observed_at IS NULL) = (succeeded IS NULL))
);
INSERT INTO relay_hosted.operation_jobs (job,stale_after_seconds) VALUES
    ('request_reconciliation',20),
    ('usage_report_cleanup',90),
    ('service_notice_cleanup',90),
    ('checkout_quote_cleanup',90),
    ('checkout_resume_cleanup',90),
    ('request_diagnostic_cleanup',90);

CREATE TABLE relay_hosted.operation_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    job text NOT NULL REFERENCES relay_hosted.operation_jobs(job),
    code text NOT NULL CHECK (code IN ('job_failed','job_recovered')),
    first_at timestamptz NOT NULL,
    last_at timestamptz NOT NULL CHECK (last_at >= first_at),
    observations integer NOT NULL CHECK (observations > 0 AND (code = 'job_failed' OR observations = 1))
);
CREATE INDEX operation_event_groups ON relay_hosted.operation_events(job,id DESC);
CREATE INDEX operation_event_expiry ON relay_hosted.operation_events(first_at);

CREATE FUNCTION relay_hosted.observe_operation(job_id text,good boolean,missed boolean)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE prior relay_hosted.operation_jobs; stamp timestamptz; cutoff timestamptz;
    gap boolean; event_code text;
BEGIN
    SELECT * INTO STRICT prior FROM relay_hosted.operation_jobs WHERE job=job_id FOR UPDATE;
    stamp:=clock_timestamp();
    cutoff:=stamp-interval '336 hours';
    gap:=missed OR coalesce(prior.observed_at > stamp
        OR prior.observed_at <= stamp-make_interval(secs=>prior.stale_after_seconds),false);
    IF NOT good THEN
        IF prior.succeeded IS FALSE AND NOT gap THEN
            UPDATE relay_hosted.operation_events e SET last_at=stamp,
                observations=least(e.observations::bigint+1,2147483647)::integer
            WHERE e.id=(SELECT id FROM relay_hosted.operation_events WHERE job=job_id ORDER BY id DESC LIMIT 1)
                AND e.code='job_failed' AND e.first_at>cutoff AND e.last_at<=stamp;
            IF NOT FOUND THEN event_code:='job_failed'; END IF;
        ELSE
            event_code:='job_failed';
        END IF;
    ELSIF prior.succeeded IS FALSE THEN
        event_code:='job_recovered';
    END IF;
    IF event_code IS NOT NULL THEN
        INSERT INTO relay_hosted.operation_events(job,code,first_at,last_at,observations)
            VALUES(job_id,event_code,stamp,stamp,1);
        WITH dropped AS (
            DELETE FROM relay_hosted.operation_events WHERE id IN (
                SELECT id FROM relay_hosted.operation_events WHERE job=job_id ORDER BY id DESC OFFSET 128
            ) RETURNING first_at
        ) SELECT gap OR EXISTS(SELECT FROM dropped WHERE first_at>cutoff) INTO gap;
    END IF;
    UPDATE relay_hosted.operation_jobs SET observed_at=stamp,succeeded=good,
        history_gap_at=CASE WHEN gap THEN stamp ELSE prior.history_gap_at END WHERE job=job_id;
END $$;

CREATE FUNCTION relay_hosted.observe_projection_cleanup(
    reports boolean,notices boolean,quotes boolean,resumes boolean,diagnostics boolean,missed boolean
) RETURNS void LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='100ms' AS $$
BEGIN
    PERFORM relay_hosted.observe_operation('usage_report_cleanup',reports,missed);
    PERFORM relay_hosted.observe_operation('service_notice_cleanup',notices,missed);
    PERFORM relay_hosted.observe_operation('checkout_quote_cleanup',quotes,missed);
    PERFORM relay_hosted.observe_operation('checkout_resume_cleanup',resumes,missed);
    PERFORM relay_hosted.observe_operation('request_diagnostic_cleanup',diagnostics,missed);
    DELETE FROM relay_hosted.operation_events WHERE id IN (
        SELECT id FROM relay_hosted.operation_events WHERE first_at<=clock_timestamp()-interval '336 hours'
        FOR UPDATE SKIP LOCKED
    );
END $$;

CREATE FUNCTION relay_hosted.observe_reconciliation(good boolean,missed boolean)
RETURNS void LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='100ms' AS $$
    SELECT relay_hosted.observe_operation('request_reconciliation',good,missed)
$$;

CREATE FUNCTION relay_hosted.operation_conditions(stamp timestamptz)
RETURNS TABLE(job text,state text,observed_at timestamptz,history_incomplete boolean)
LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT j.job,condition.state,j.observed_at,
        condition.state IN ('unobserved','stale')
            OR coalesce(j.history_gap_at > stamp-interval '336 hours',false)
    FROM relay_hosted.operation_jobs j
    CROSS JOIN LATERAL (SELECT CASE
        WHEN j.observed_at IS NULL THEN 'unobserved'
        WHEN j.observed_at>stamp OR j.observed_at<=stamp-make_interval(secs=>j.stale_after_seconds) THEN 'stale'
        WHEN j.succeeded THEN 'succeeded'
        ELSE 'failed' END AS state) condition
$$;

CREATE FUNCTION relay_hosted.owner_operations_status(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=statement_timestamp(); result jsonb;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    SELECT jsonb_build_object('version',1,'as_of_ms',floor(extract(epoch FROM stamp)*1000)::bigint,
        'jobs',jsonb_agg(jsonb_build_object('job',c.job,'state',c.state,
            'observed_at_ms',floor(extract(epoch FROM c.observed_at)*1000)::bigint) ORDER BY c.job COLLATE "C"))
    INTO result FROM relay_hosted.operation_conditions(stamp) c;
    RETURN result;
END $$;

CREATE FUNCTION relay_hosted.owner_operations_events(origin text,tag bytea,prior_id bigint)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=statement_timestamp(); cutoff timestamptz:=stamp-interval '336 hours'; result jsonb;
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF prior_id IS NOT NULL AND NOT EXISTS (
        SELECT FROM relay_hosted.operation_events WHERE id=prior_id AND first_at>cutoff AND last_at<=stamp
    ) THEN RETURN '{"error":"cursor_unavailable"}'; END IF;
    WITH page AS (
        SELECT * FROM relay_hosted.operation_events
        WHERE first_at>cutoff AND last_at<=stamp AND (prior_id IS NULL OR id<prior_id)
        ORDER BY id DESC LIMIT 25
    ), numbered AS (
        SELECT p.*,row_number() OVER (ORDER BY id DESC) AS position FROM page p
    )
    SELECT jsonb_build_object('version',1,'partial',EXISTS(
        SELECT FROM relay_hosted.operation_conditions(stamp) c WHERE c.history_incomplete),
        'items',coalesce(jsonb_agg(jsonb_build_object('id',id::text,'job',job,'code',code,
            'first_at_ms',floor(extract(epoch FROM first_at)*1000)::bigint,
            'last_at_ms',floor(extract(epoch FROM last_at)*1000)::bigint,
            'observations',observations) ORDER BY id DESC) FILTER(WHERE position<=24),'[]'),
        'next_before',CASE WHEN count(*)>24 THEN min(id::text) FILTER(WHERE position=24) END)
    INTO result FROM numbered;
    RETURN result;
END $$;

REVOKE EXECUTE ON FUNCTION relay_hosted.observe_operation(text,boolean,boolean),
    relay_hosted.observe_projection_cleanup(boolean,boolean,boolean,boolean,boolean,boolean),
    relay_hosted.observe_reconciliation(boolean,boolean),
    relay_hosted.operation_conditions(timestamptz),
    relay_hosted.owner_operations_status(text,bytea),
    relay_hosted.owner_operations_events(text,bytea,bigint) FROM PUBLIC;
