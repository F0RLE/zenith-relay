CREATE TABLE relay_hosted.desktop_source_copies (
    operation_id uuid PRIMARY KEY,
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    device_id uuid NOT NULL REFERENCES relay_hosted.management_devices,
    source_id uuid NOT NULL,
    plan jsonb NOT NULL,
    verifier bytea NOT NULL CHECK (octet_length(verifier)=32),
    commitment bytea NOT NULL CHECK (octet_length(commitment)=32),
    approved_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at=approved_at+interval '10 minutes'),
    committed_at timestamptz CHECK (committed_at>=approved_at AND committed_at<expires_at)
);
CREATE INDEX desktop_copy_source ON relay_hosted.desktop_source_copies(installation_id,source_id);
CREATE UNIQUE INDEX desktop_copy_saved ON relay_hosted.desktop_source_copies(installation_id,source_id)
    WHERE committed_at IS NOT NULL;
CREATE INDEX desktop_copy_pending ON relay_hosted.desktop_source_copies(installation_id,device_id,expires_at)
    WHERE committed_at IS NULL;

-- Invoked by the web role; only the owner can create an exact, time-limited write grant.
CREATE FUNCTION relay_hosted.review_source_copy(origin text,tag bytea,iid uuid,device uuid,op uuid,source uuid,
    spec jsonb,proof bytea,payload bytea,approve boolean)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor uuid; ie bigint; ue bigint; signed_at timestamptz; stamp timestamptz;
    d relay_hosted.management_devices; c relay_hosted.desktop_source_copies; result text;
BEGIN
    IF origin NOT LIKE 'https://%' OR octet_length(spec::text)>2048
        OR NOT spec @> jsonb_build_object('origin',origin,'installation_id',iid,'device_id',device,'operation_id',op,'source_id',source)
        OR octet_length(proof)<>32 OR octet_length(payload)<>32 THEN RETURN '{"state":"invalid"}'; END IF;
    IF approve THEN PERFORM pg_advisory_xact_lock(hashtextextended('relay-source-copy:'||iid::text,0)); END IF;
    SELECT a.user_id,a.installation_epoch,a.user_epoch,s.created_at INTO actor,ie,ue,signed_at
        FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.admin_sessions s
        ON (s.user_id,s.credential_id,s.installation_epoch,s.user_epoch)=(a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
        JOIN relay_hosted.installations i ON i.public_origin=a.public_origin
        WHERE i.id=iid AND a.public_origin=origin AND s.verifier=tag AND s.expires_at>clock_timestamp()
        AND s.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF s,a;
    IF NOT FOUND THEN RETURN '{"state":"denied"}'; END IF;
    SELECT * INTO d FROM relay_hosted.management_devices WHERE id=device AND installation_id=iid
        AND owner_id=actor AND installation_epoch=ie AND owner_epoch=ue FOR SHARE;
    stamp:=clock_timestamp();
    IF d.id IS NULL THEN RETURN '{"state":"conflict"}'; END IF;
    SELECT * INTO c FROM relay_hosted.desktop_source_copies WHERE operation_id=op;
    IF FOUND THEN
        IF (c.installation_id,c.device_id,c.source_id,c.plan,c.verifier,c.commitment)<>(iid,device,source,spec,proof,payload)
            THEN RETURN '{"state":"conflict"}'; END IF;
        result:=CASE WHEN c.committed_at IS NOT NULL THEN 'committed'
            WHEN c.expires_at<=stamp OR d.revoked_at IS NOT NULL OR coalesce(d.expires_at<=stamp,true) THEN 'expired'
            ELSE 'approved' END;
    ELSIF d.revoked_at IS NOT NULL OR coalesce(d.expires_at<=stamp,true)
        OR EXISTS(SELECT FROM relay_hosted.connection_secret_slots WHERE installation_id=iid AND connection_id=source)
        OR EXISTS(SELECT FROM relay_hosted.static_source_provisions r WHERE r.installation_id=iid AND r.operation_id=op)
        OR EXISTS(SELECT FROM relay_hosted.desktop_source_copies f WHERE f.installation_id=iid AND f.source_id=source
            AND f.expires_at>stamp AND f.committed_at IS NULL)
        THEN RETURN '{"state":"conflict"}';
    ELSIF NOT approve THEN result:='awaiting_approval';
    ELSIF signed_at<=stamp-interval '2 minutes' THEN RETURN '{"state":"fresh_required"}';
    ELSIF (SELECT count(*) FROM relay_hosted.desktop_source_copies f WHERE f.installation_id=iid
            AND f.expires_at>stamp AND f.committed_at IS NULL)>=32 THEN RETURN '{"state":"limit"}';
    ELSE
        INSERT INTO relay_hosted.desktop_source_copies(operation_id,installation_id,device_id,source_id,plan,verifier,commitment,approved_at,expires_at)
            VALUES(op,iid,device,source,spec,proof,payload,stamp,stamp+interval '10 minutes');
        result:='approved';
    END IF;
    RETURN jsonb_build_object('state',result,'device_label',d.label,'plan',spec);
END $$;

-- The vault role alone sees the commitment. During apply hold the device and
-- authority fences, and serialize same-operation attempts until the transaction ends.
CREATE FUNCTION relay_hosted.source_copy_access(origin text,iid uuid,device uuid,op uuid,proof bytea,applying boolean)
RETURNS TABLE(state text,plan jsonb,payload_tag bytea)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE c relay_hosted.desktop_source_copies; active boolean;
BEGIN
    IF NOT EXISTS (SELECT FROM relay_hosted.installations h WHERE h.id=iid AND h.public_origin=origin)
        THEN RETURN; END IF;
    IF applying THEN
        SELECT * INTO c FROM relay_hosted.desktop_source_copies k WHERE k.operation_id=op FOR UPDATE;
    ELSE
        SELECT * INTO c FROM relay_hosted.desktop_source_copies k WHERE k.operation_id=op;
    END IF;
    IF c.operation_id IS NOT NULL AND (c.installation_id,c.device_id,c.verifier)<>(iid,device,proof)
        THEN RETURN; END IF;
    IF c.committed_at IS NOT NULL THEN
        IF c.committed_at>clock_timestamp()-interval '7 days' THEN
            RETURN QUERY SELECT 'committed'::text,NULL::jsonb,NULL::bytea;
        END IF;
        RETURN;
    END IF;
    IF applying THEN
        PERFORM 1 FROM relay_hosted.management_devices d JOIN relay_hosted.active_owner_credentials a
            ON (a.public_origin,a.user_id,a.installation_epoch,a.user_epoch)=(origin,d.owner_id,d.installation_epoch,d.owner_epoch)
            WHERE d.id=device AND d.installation_id=iid AND d.revoked_at IS NULL AND d.expires_at>clock_timestamp()
            FOR SHARE OF d,a;
        active:=FOUND;
    ELSE
        SELECT EXISTS (SELECT FROM relay_hosted.management_devices d JOIN relay_hosted.active_owner_credentials a
            ON (a.public_origin,a.user_id,a.installation_epoch,a.user_epoch)=(origin,d.owner_id,d.installation_epoch,d.owner_epoch)
            WHERE d.id=device AND d.installation_id=iid AND d.revoked_at IS NULL AND d.expires_at>clock_timestamp()) INTO active;
    END IF;
    IF c.operation_id IS NULL THEN
        IF active THEN RETURN QUERY SELECT 'awaiting_approval'::text,NULL::jsonb,NULL::bytea; END IF;
    ELSIF NOT active OR c.expires_at<=clock_timestamp() THEN
        RETURN QUERY SELECT 'expired'::text,NULL::jsonb,NULL::bytea;
    ELSE
        RETURN QUERY SELECT 'approved'::text,c.plan,c.commitment;
    END IF;
END $$;

CREATE FUNCTION relay_hosted.finish_source_copy(origin text,iid uuid,device uuid,op uuid,proof bytea)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz;
BEGIN
    PERFORM 1 FROM relay_hosted.source_copy_access(origin,iid,device,op,proof,true) a WHERE a.state='approved';
    IF NOT FOUND THEN RETURN false; END IF;
    stamp:=clock_timestamp();
    UPDATE relay_hosted.desktop_source_copies c SET committed_at=stamp
        WHERE c.operation_id=op AND c.expires_at>stamp AND c.committed_at IS NULL
        AND EXISTS(SELECT FROM relay_hosted.static_source_provisions p WHERE
            (p.installation_id,p.operation_id,p.source_id,p.proxied)=(iid,op,c.source_id,c.plan->'proxy'<>'null'::jsonb));
    RETURN FOUND;
END $$;

CREATE FUNCTION relay_hosted.source_copy_route(iid uuid,source uuid,endpoint text,proxied boolean)
RETURNS boolean LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT NOT EXISTS(SELECT FROM relay_hosted.desktop_source_copies c
        WHERE c.installation_id=$1 AND c.source_id=$2 AND c.committed_at IS NOT NULL
        AND (c.plan->>'endpoint',c.plan->'proxy'<>'null'::jsonb) IS DISTINCT FROM ($3,$4))
$$;

CREATE OR REPLACE FUNCTION relay_hosted.connection_page(iid uuid,after_id uuid) RETURNS jsonb
LANGUAGE sql SET search_path=pg_catalog AS $$
    WITH page AS MATERIALIZED (
        SELECT s.connection_id,s.generation IS NOT NULL AS stored FROM relay_hosted.connection_secret_slots s
        WHERE s.installation_id=iid AND s.purpose='provider' AND (after_id IS NULL OR s.connection_id>after_id)
        ORDER BY s.connection_id LIMIT 33
    ), visible AS (SELECT * FROM page ORDER BY connection_id LIMIT 32)
    SELECT jsonb_build_object('items',coalesce(jsonb_agg(jsonb_build_object(
        'id',v.connection_id,'label',coalesce(c.plan->>'label',(SELECT i.source_label FROM relay_hosted.text_inventory i
            WHERE i.source_id=v.connection_id ORDER BY i.checked_at DESC,i.id DESC LIMIT 1)),
        'credential_state',CASE WHEN v.stored THEN 'stored' ELSE 'revoked' END,'proxy_bound',p.generation IS NOT NULL,
        'verified_models',ARRAY(SELECT DISTINCT i.model FROM relay_hosted.current_text_inventory i
            WHERE i.installation_id=iid AND i.source_id=v.connection_id ORDER BY i.model)
    ) ORDER BY v.connection_id),'[]'::jsonb),'next_after',CASE WHEN (SELECT count(*) FROM page)=33
        THEN (SELECT connection_id FROM visible ORDER BY connection_id DESC LIMIT 1) END)
    FROM visible v LEFT JOIN relay_hosted.connection_secret_slots p
        ON (p.installation_id,p.connection_id,p.purpose)=(iid,v.connection_id,'proxy')
    LEFT JOIN relay_hosted.desktop_source_copies c
        ON (c.installation_id,c.source_id)=(iid,v.connection_id) AND c.committed_at IS NOT NULL
$$;

REVOKE ALL ON FUNCTION relay_hosted.review_source_copy(text,bytea,uuid,uuid,uuid,uuid,jsonb,bytea,bytea,boolean),
    relay_hosted.source_copy_access(text,uuid,uuid,uuid,bytea,boolean),
    relay_hosted.finish_source_copy(text,uuid,uuid,uuid,bytea),relay_hosted.source_copy_route(uuid,uuid,text,boolean) FROM PUBLIC;
