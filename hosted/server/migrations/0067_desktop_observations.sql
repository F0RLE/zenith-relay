ALTER TABLE relay_hosted.management_devices ADD COLUMN read_scopes text[] NOT NULL DEFAULT '{}'
    CHECK (read_scopes IN ('{}', '{overview.read}', '{usage.read}', '{overview.read,usage.read}'));

CREATE OR REPLACE FUNCTION relay_hosted.owner_devices(origin text,tag bytea) RETURNS jsonb
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE h relay_hosted.installations; stamp timestamptz:=statement_timestamp();
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    SELECT * INTO STRICT h FROM relay_hosted.installations WHERE public_origin=origin;
    RETURN (WITH rows AS (
        SELECT d.*, CASE WHEN revoked_at IS NOT NULL THEN 'revoked'
            WHEN coalesce(expires_at,approve_by)<=stamp THEN 'expired'
            WHEN approved_at IS NULL THEN 'pending' ELSE 'active' END AS state
        FROM relay_hosted.management_devices d JOIN relay_hosted.users u ON u.id=d.owner_id
        WHERE d.installation_id=h.id AND d.owner_id=h.owner_user_id AND d.installation_epoch=h.authorization_epoch
            AND d.owner_epoch=u.authorization_epoch
        ORDER BY (revoked_at IS NULL AND coalesce(expires_at,approve_by)>stamp) DESC,invited_at DESC,id LIMIT 32
    ) SELECT jsonb_build_object('installation_id',h.id,'origin',origin,'items',coalesce(jsonb_agg(jsonb_build_object(
        'id',id,'label',label,'scope',scope,'read_scopes',read_scopes,'revision',revision::text,'state',state,
        'expires_at_s',floor(extract(epoch FROM coalesce(expires_at,approve_by)))::bigint,
        'last_used_at_s',floor(extract(epoch FROM last_used_at))::bigint)
        ORDER BY (state IN ('pending','active')) DESC,invited_at DESC,id),'[]'::jsonb)) FROM rows);
END $$;

DROP FUNCTION relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea,bytea,bigint,bigint,boolean,boolean,bigint);

CREATE FUNCTION relay_hosted.change_device(origin text,tag bytea,op uuid,device uuid,rev bigint,act text,name text,proof bytea,
    scopes text[],proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; iid uuid; stamp timestamptz; proof_result text; confirmer uuid;
    d relay_hosted.management_devices; prior relay_hosted.device_changes;
BEGIN
    IF op IS NULL OR device IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR device='00000000-0000-0000-0000-000000000000'
        OR rev IS NULL OR rev NOT BETWEEN 0 AND 2 OR act IS NULL OR (op=device)<>(act='invite')
        OR act NOT IN ('invite','approve','revoke') OR (act='invite')<>(name IS NOT NULL)
        OR (act='approve')<>(proof IS NOT NULL) OR octet_length(proof)<>32
        OR octet_length(name) NOT BETWEEN 1 AND 64 OR name<>btrim(name) OR name ~ '[[:cntrl:]]'
        OR scopes IS NULL OR scopes NOT IN ('{}', '{overview.read}', '{usage.read}', '{overview.read,usage.read}')
        OR (act='revoke' AND scopes<>'{}')
        THEN RETURN 'invalid'; END IF;
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    PERFORM pg_advisory_xact_lock(hashtextextended('relay-devices:'||iid::text,0));
    SELECT * INTO d FROM relay_hosted.management_devices WHERE id=device AND installation_id=iid
        AND owner_id=ss.user_id AND installation_epoch=ss.installation_epoch AND owner_epoch=ss.user_epoch FOR UPDATE;
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.device_changes WHERE operation=op;
    IF FOUND THEN
        IF d.id IS NOT NULL AND (prior.device_id,prior.action,prior.expected)=(device,act,rev)
            AND (act='revoke' OR d.read_scopes=scopes)
            AND (name IS NULL OR name=d.label) AND (proof IS NULL OR proof=d.verifier) THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF proof_credential IS NOT NULL
        AND relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true THEN RETURN 'passkey_changed'; END IF;
    stamp:=clock_timestamp();
    IF act='invite' THEN
        IF d.id IS NOT NULL OR rev<>0 OR EXISTS(SELECT FROM relay_hosted.management_devices WHERE id=device)
            THEN RETURN 'conflict'; END IF;
        IF (SELECT count(*) FROM relay_hosted.management_devices WHERE installation_id=iid AND owner_id=ss.user_id
            AND installation_epoch=ss.installation_epoch AND owner_epoch=ss.user_epoch
            AND revoked_at IS NULL AND coalesce(expires_at,approve_by)>stamp)>=16 THEN RETURN 'limit'; END IF;
    ELSIF d.id IS NULL OR rev<>d.revision OR d.scope<>'connections.read' OR d.revoked_at IS NOT NULL
        OR coalesce(d.expires_at,d.approve_by)<=stamp
        OR (act='approve' AND (d.approved_at IS NOT NULL OR d.read_scopes<>scopes))
        THEN RETURN 'conflict';
    END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    SELECT outcome,confirmed.confirmer,verified_at INTO proof_result,confirmer,stamp
        FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,
            proof_backup,proof_eligible,proof_deadline_ms) confirmed;
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    IF act<>'invite' AND coalesce(d.expires_at,d.approve_by)<=stamp THEN RETURN 'conflict'; END IF;

    IF act='invite' THEN
        INSERT INTO relay_hosted.management_devices(id,installation_id,owner_id,installation_epoch,owner_epoch,label,scope,
            read_scopes,invited_at,approve_by)
            VALUES(device,iid,ss.user_id,ss.installation_epoch,ss.user_epoch,name,'connections.read',scopes,stamp,stamp+interval '5 minutes');
    ELSIF act='approve' THEN
        UPDATE relay_hosted.management_devices SET approved_at=stamp,expires_at=stamp+interval '24 hours',
            verifier=proof,revision=revision+1 WHERE id=device;
    ELSE
        UPDATE relay_hosted.management_devices SET revoked_at=stamp,revision=revision+1 WHERE id=device;
    END IF;
    INSERT INTO relay_hosted.device_changes(operation,device_id,action,expected,at,confirmed_by_passkey_id)
        VALUES(op,device,act,rev,stamp,confirmer);
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.device_read_context(origin text,iid uuid,device uuid,proof bytea,required_scope text)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('installation_id',iid,'device_id',device,
        'expires_at_s',floor(extract(epoch FROM d.expires_at))::bigint,'result',d.read_scopes)
    FROM relay_hosted.management_devices d JOIN relay_hosted.installations h ON h.id=d.installation_id
    WHERE h.id=iid AND h.public_origin=origin AND d.id=device AND d.verifier=proof
        AND d.scope='connections.read' AND d.revoked_at IS NULL AND d.expires_at>statement_timestamp()
        AND (required_scope IS NULL OR required_scope=ANY(d.read_scopes))
        AND EXISTS(SELECT FROM relay_hosted.active_owner_credentials a WHERE a.public_origin=origin
            AND (a.user_id,a.installation_epoch,a.user_epoch)=(d.owner_id,d.installation_epoch,d.owner_epoch))
$$;

CREATE FUNCTION relay_hosted.device_access(origin text,iid uuid,device uuid,proof bytea)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT relay_hosted.device_read_context(origin,iid,device,proof,NULL)
$$;

CREATE FUNCTION relay_hosted.device_overview(origin text,iid uuid,device uuid,proof bytea,hours integer)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctx jsonb; stamp timestamptz:=date_trunc('milliseconds',statement_timestamp()); result jsonb;
BEGIN
    ctx:=relay_hosted.device_read_context(origin,iid,device,proof,'overview.read');
    IF ctx IS NULL THEN RETURN NULL; END IF;
    IF hours NOT IN (24,168,720) THEN RETURN '{"error":"invalid_filter"}'; END IF;
    -- Match platform_usage_overview's retained request and token evidence. No journal joins.
    WITH measured AS (
        SELECT r.execution,coalesce(l.input_tokens::numeric+l.output_tokens,r.input_tokens::numeric+r.output_tokens) AS tokens,
            l.request_id IS NULL AND r.basis IS DISTINCT FROM 'provider_final'
                AND (r.input_tokens IS NOT NULL OR r.output_tokens IS NOT NULL) AS partial
        FROM relay_hosted.text_requests r LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=r.id
        WHERE r.started_at>=stamp-hours*interval '1 hour' AND r.started_at<stamp
    ), active AS (
        SELECT count(*) AS requests,count(*) FILTER (WHERE execution='dispatched' AND claimed_at IS NOT NULL) AS attempts
        FROM relay_hosted.text_requests
        WHERE closed_at IS NULL AND close_by>stamp AND
            (execution='dispatched' OR (execution='reserved' AND claim_by>stamp))
    )
    SELECT jsonb_build_object('hours',hours,'as_of_ms',(extract(epoch FROM stamp)*1000)::bigint,
        'requests',count(*)::text,'token_samples',count(tokens)::text,'partial_tokens',count(*) FILTER (WHERE partial)::text,
        'total_tokens',CASE WHEN count(*)=0 THEN '0' ELSE sum(tokens)::text END,
        'errors',count(*) FILTER (WHERE execution IN ('incomplete','failed','uncertain'))::text,
        'active_requests',(SELECT requests::text FROM active),'active_attempts',(SELECT attempts::text FROM active))
    INTO result FROM measured;
    RETURN ctx||jsonb_build_object('result',result);
END $$;

CREATE FUNCTION relay_hosted.device_usage(origin text,iid uuid,device uuid,proof bytea,hours integer,
    model_filter text,execution_filter text,as_of_ms bigint,before_id uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctx jsonb; stamp timestamptz:=date_trunc('milliseconds',statement_timestamp()); start_at timestamptz;
    end_at timestamptz; cursor_at timestamptz; now_ms bigint:=(extract(epoch FROM stamp)*1000)::bigint; result jsonb;
BEGIN
    ctx:=relay_hosted.device_read_context(origin,iid,device,proof,'usage.read');
    IF ctx IS NULL THEN RETURN NULL; END IF;
    IF hours IS NULL OR hours NOT IN (24,168,720)
        OR model_filter COLLATE "C" !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$'
        OR execution_filter NOT IN ('reserved','dispatched','completed','incomplete','failed','uncertain')
        OR (before_id IS NOT NULL AND as_of_ms IS NULL)
        OR as_of_ms NOT BETWEEN now_ms-900000 AND now_ms THEN RETURN '{"error":"invalid_filter"}'; END IF;
    as_of_ms:=coalesce(as_of_ms,now_ms);
    end_at:=stamp-(now_ms-as_of_ms)*interval '1 millisecond';
    start_at:=end_at-hours*interval '1 hour';
    IF before_id IS NOT NULL THEN
        SELECT r.started_at INTO cursor_at FROM relay_hosted.text_requests r
            WHERE r.id=before_id AND r.started_at>=start_at AND r.started_at<end_at;
        IF NOT FOUND THEN RETURN '{"error":"invalid_filter"}'; END IF;
    END IF;
    WITH page AS MATERIALIZED (
        SELECT r.id,r.started_at,o.model,r.execution,r.basis,r.input_tokens,r.output_tokens
        FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
        WHERE r.started_at>=start_at AND r.started_at<end_at
            AND (cursor_at IS NULL OR (r.started_at,r.id)<(cursor_at,before_id))
            AND (model_filter IS NULL OR o.model=model_filter COLLATE "C")
            AND (execution_filter IS NULL OR r.execution=execution_filter)
        ORDER BY r.started_at DESC,r.id DESC LIMIT 33
    ), visible AS (
        SELECT * FROM page ORDER BY started_at DESC,id DESC LIMIT 32
    )
    SELECT jsonb_build_object('hours',hours,'as_of_ms',as_of_ms,'model',model_filter,'execution',execution_filter,
        'items',coalesce(jsonb_agg(jsonb_build_object('request_id',v.id,
            'started_at_ms',floor(extract(epoch FROM v.started_at)*1000)::bigint,'model',v.model,'execution',v.execution,
            'tokens',coalesce(l.input_tokens::numeric+l.output_tokens,v.input_tokens::numeric+v.output_tokens)::text,
            'partial_tokens',l.request_id IS NULL AND v.basis IS DISTINCT FROM 'provider_final'
                AND (v.input_tokens IS NOT NULL OR v.output_tokens IS NOT NULL),
            'diagnostic_code',(SELECT d.code FROM relay_hosted.request_diagnostics d
                WHERE d.request_id=v.id AND d.expires_at>stamp ORDER BY d.sequence DESC LIMIT 1))
            ORDER BY v.started_at DESC,v.id DESC),'[]'),
        'next_before',CASE WHEN (SELECT count(*) FROM page)=33
            THEN (SELECT id FROM visible ORDER BY started_at,id LIMIT 1) END)
    INTO result FROM visible v LEFT JOIN relay_hosted.text_late_usage l ON l.request_id=v.id;
    RETURN ctx||jsonb_build_object('result',result);
END $$;

REVOKE ALL ON FUNCTION
    relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea,text[],bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.device_read_context(text,uuid,uuid,bytea,text),
    relay_hosted.device_access(text,uuid,uuid,bytea),
    relay_hosted.device_overview(text,uuid,uuid,bytea,integer),
    relay_hosted.device_usage(text,uuid,uuid,bytea,integer,text,text,bigint,uuid)
    FROM PUBLIC;
