ALTER TABLE relay_hosted.service_notice_operations
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    ADD CHECK (confirmed_by_passkey_id IS NULL OR action='publish');
ALTER TABLE relay_hosted.diagnostic_capture_audit
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    ADD CHECK (confirmed_by_passkey_id IS NULL OR action='start');

DROP FUNCTION relay_hosted.change_service_notice(text,bytea,uuid,uuid,integer,text,text,text);
DROP FUNCTION relay_hosted.start_diagnostic_capture(
    text,bytea,uuid,uuid,relay_hosted.capture_reason,integer,integer,integer,integer,integer);

CREATE FUNCTION relay_hosted.change_service_notice(origin text,tag bytea,nid uuid,op uuid,expected integer,
    verb text,heading text,body text,proof_credential bytea,proof_revision bigint,proof_counter bigint,
    proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctl relay_hosted.service_control; ss relay_hosted.admin_sessions; stamp timestamptz;
    prior relay_hosted.service_notice_operations; note relay_hosted.service_notices; digest bytea;
    audience uuid[]; scope_until timestamptz; proof_result text; confirmer uuid;
BEGIN
    IF nid IS NULL OR nid='00000000-0000-0000-0000-000000000000' OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 0 AND 2147483646 OR verb IS NULL
        OR (verb='save' AND (heading IS NULL OR body IS NULL OR char_length(heading) NOT BETWEEN 1 AND 120
            OR char_length(body) NOT BETWEEN 1 AND 4000 OR btrim(heading)='' OR btrim(body)=''))
        OR (verb<>'save' AND (verb NOT IN ('publish','discard') OR expected=0 OR heading IS NOT NULL OR body IS NOT NULL))
        OR (verb<>'publish' AND proof_credential IS NOT NULL) THEN RETURN 'invalid'; END IF;
    IF (relay_hosted.owner_passkey_session(origin,tag)).verifier IS NULL THEN RETURN 'denied'; END IF;

    -- Match customer scope changes and owner recovery before freezing the
    -- bounded audience. The passkey wait happens outside this transaction.
    SELECT s.* INTO ctl FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
        WHERE i.singleton AND i.public_origin=origin AND i.setup_state='claimed'
            AND s.restore_operation IS NULL FOR UPDATE OF i,s;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;

    stamp:=clock_timestamp();
    digest:=sha256(convert_to(jsonb_build_array(heading,body)::text,'UTF8'));
    SELECT * INTO prior FROM relay_hosted.service_notice_operations h WHERE h.operation_id=op;
    IF FOUND THEN
        RETURN CASE WHEN (prior.actor_id,prior.notice_id,prior.action,prior.expected_revision,prior.content_digest)
            =(ss.user_id,nid,verb,expected,digest) THEN 'applied' ELSE 'conflict' END;
    END IF;
    IF (SELECT count(*) FROM relay_hosted.service_notice_operations)>=2048 THEN RETURN 'capacity'; END IF;
    SELECT * INTO note FROM relay_hosted.service_notices n WHERE n.id=nid FOR UPDATE;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    IF expected=0 AND NOT FOUND THEN
        -- A discarded draft cannot be replaced under an outstanding challenge
        -- for the same ID and revision. Its receipt outlives every ceremony.
        IF EXISTS(SELECT FROM relay_hosted.service_notice_operations h WHERE h.notice_id=nid) THEN RETURN 'conflict'; END IF;
        IF (SELECT count(*) FROM relay_hosted.service_notices WHERE published_at IS NULL)>=16 THEN RETURN 'capacity'; END IF;
        INSERT INTO relay_hosted.service_notices(id,revision,title,message,updated_at)
            VALUES(nid,1,heading,body,stamp);
    ELSIF NOT FOUND OR note.published_at IS NOT NULL OR note.revision<>expected THEN
        RETURN 'conflict';
    ELSIF verb='save' THEN
        UPDATE relay_hosted.service_notices SET title=heading,message=body,revision=expected+1,updated_at=stamp WHERE id=nid;
    ELSIF verb='discard' THEN
        DELETE FROM relay_hosted.service_notices WHERE id=nid;
    ELSE
        IF (SELECT count(*) FROM relay_hosted.service_notices WHERE expires_at>stamp)>=32 THEN RETURN 'capacity'; END IF;
        IF ctl.mode<>'qualification' THEN RETURN 'no_audience'; END IF;
        SELECT expires_at INTO scope_until FROM relay_hosted.configuration_revisions WHERE id=ctl.qualification_scope_ref FOR SHARE;
        IF scope_until IS NULL OR scope_until<=stamp THEN RETURN 'no_audience'; END IF;
        SELECT array_agg(eligible.id) INTO audience FROM (
            SELECT w.id FROM relay_hosted.qualification_subjects q
            JOIN relay_hosted.configuration_revisions r ON r.id=q.scope_id
            JOIN relay_hosted.workspaces w ON w.id=q.workspace_id
            JOIN relay_hosted.users u ON u.id=w.owner_user_id
            JOIN relay_hosted.external_identities e ON e.provider_id=r.provider_id
              AND e.subject_digest=q.subject_digest AND e.user_id=u.id
            WHERE q.scope_id=ctl.qualification_scope_ref AND u.disabled_at IS NULL AND e.revoked_at IS NULL
            ORDER BY w.id FOR SHARE OF w,u,e
        ) eligible;
        IF audience IS NULL THEN RETURN 'no_audience'; END IF;
        stamp:=clock_timestamp();
        IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
        IF scope_until<=stamp THEN RETURN 'no_audience'; END IF;
        IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
        SELECT outcome,confirmed.confirmer,verified_at INTO proof_result,confirmer,stamp
            FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,
                proof_backup,proof_eligible,proof_deadline_ms) confirmed;
        IF proof_result<>'verified' THEN RETURN proof_result; END IF;
        IF scope_until<=stamp THEN RETURN 'no_audience'; END IF;

        INSERT INTO relay_hosted.service_notice_inbox(notice_id,workspace_id,user_id)
            SELECT nid,w.id,w.owner_user_id FROM relay_hosted.workspaces w WHERE w.id=ANY(audience) ORDER BY w.id;
        UPDATE relay_hosted.service_notices SET revision=expected+1,updated_at=stamp,published_at=stamp,
            expires_at=stamp+interval '720 hours',scope_id=ctl.qualification_scope_ref,
            recipient_count=cardinality(audience) WHERE id=nid;
    END IF;
    INSERT INTO relay_hosted.service_notice_operations(
        operation_id,actor_id,notice_id,action,expected_revision,content_digest,applied_at,confirmed_by_passkey_id)
        VALUES(op,ss.user_id,nid,verb,expected,digest,stamp,confirmer);
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.start_diagnostic_capture(
    p_origin text,p_tag bytea,p_operation uuid,p_request uuid,p_reason relay_hosted.capture_reason,
    seconds integer,max_seconds integer,event_limit integer,byte_limit integer,rate_limit integer,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint
) RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog SET lock_timeout='150ms' AS $$
DECLARE ss relay_hosted.admin_sessions; d relay_hosted.diagnostic_sessions; prior uuid;
    stamp timestamptz; cause relay_hosted.capture_stop; request_deadline timestamptz;
    proof_result text; confirmer uuid;
BEGIN
    IF relay_hosted.owner_usage_session(p_origin,p_tag) IS NOT TRUE THEN RETURN NULL; END IF;
    IF p_operation IS NULL OR p_operation='00000000-0000-0000-0000-000000000000'
        OR p_request IS NULL OR p_request='00000000-0000-0000-0000-000000000000'
        OR p_reason IS NULL OR seconds IS NULL OR seconds NOT BETWEEN 1 AND 1800
    THEN RETURN '{"error":"invalid_capture"}'; END IF;

    PERFORM pg_advisory_xact_lock(65082942);
    SELECT session_id INTO prior FROM relay_hosted.diagnostic_capture_audit WHERE operation_id=p_operation;
    IF prior IS NOT NULL THEN
        SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE id=prior;
    ELSE
        -- Like stop and append, acquire the capture row before owner locks.
        SELECT * INTO d FROM relay_hosted.diagnostic_sessions WHERE ended_at IS NULL FOR UPDATE;
    END IF;
    ss:=relay_hosted.lock_owner_payment_session(p_origin,p_tag);
    IF ss.verifier IS NULL THEN RETURN NULL; END IF;
    stamp:=clock_timestamp();
    IF prior IS NOT NULL THEN
        IF d.id IS NULL OR coalesce(d.ended_at,d.expires_at)<=stamp-interval '336 hours'
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
    IF d.id IS NOT NULL THEN
        cause:=relay_hosted.capture_stop_condition(d,stamp);
        IF cause IS NULL THEN RETURN '{"error":"capture_busy"}'; END IF;
    END IF;
    IF (SELECT count(*) FROM relay_hosted.diagnostic_capture_audit WHERE action='start')>=1024
    THEN RETURN '{"error":"capture_history_full"}'; END IF;

    -- Fence concurrent completion; the request also has a wall-clock deadline.
    SELECT r.close_by INTO request_deadline FROM relay_hosted.text_requests r WHERE r.id=p_request
        AND r.started_at<=stamp AND r.closed_at IS NULL AND r.execution IN ('reserved','dispatched')
        FOR SHARE OF r;
    stamp:=clock_timestamp();
    IF request_deadline IS NULL OR request_deadline<=stamp THEN RETURN '{"error":"capture_request_unavailable"}'; END IF;
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN NULL; END IF;
    IF proof_credential IS NULL THEN RETURN '{"error":"fresh_required"}'; END IF;
    SELECT outcome,confirmed.confirmer,verified_at INTO proof_result,confirmer,stamp
        FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,
            proof_backup,proof_eligible,proof_deadline_ms) confirmed;
    IF proof_result='denied' THEN RETURN NULL; END IF;
    IF proof_result<>'verified' THEN RETURN jsonb_build_object('error',proof_result); END IF;
    IF request_deadline<=stamp THEN RETURN '{"error":"capture_request_unavailable"}'; END IF;

    IF d.id IS NOT NULL THEN PERFORM relay_hosted.close_diagnostic_capture(d.id,cause,stamp); END IF;
    INSERT INTO relay_hosted.diagnostic_sessions(operation_id,request_id,origin,proof_tag,reason,started_at,expires_at,
        max_events,max_bytes,max_rate)
    VALUES(p_operation,p_request,p_origin,p_tag,p_reason,stamp,stamp+make_interval(secs=>seconds),
        event_limit,byte_limit,rate_limit) RETURNING * INTO d;
    INSERT INTO relay_hosted.diagnostic_capture_audit(
        session_id,action,operation_id,actor_id,at,reason,confirmed_by_passkey_id)
        VALUES(d.id,'start',p_operation,ss.user_id,stamp,p_reason,confirmer);
    RETURN jsonb_build_object('version',1,'as_of_ms',relay_hosted.capture_millis(stamp),
        'session',relay_hosted.capture_summary(d,stamp));
END $$;

REVOKE ALL ON FUNCTION
    relay_hosted.change_service_notice(text,bytea,uuid,uuid,integer,text,text,text,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.start_diagnostic_capture(text,bytea,uuid,uuid,relay_hosted.capture_reason,integer,integer,integer,integer,integer,
        bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
