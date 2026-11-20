CREATE TABLE relay_hosted.management_devices (
    id uuid PRIMARY KEY, installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    owner_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    installation_epoch bigint NOT NULL, owner_epoch bigint NOT NULL,
    label text NOT NULL CHECK (octet_length(label) BETWEEN 1 AND 64 AND label=btrim(label) AND label !~ '[[:cntrl:]]'),
    scope text NOT NULL CHECK(scope='connections.read'),
    revision bigint NOT NULL DEFAULT 1 CHECK(revision BETWEEN 1 AND 3),
    invited_at timestamptz NOT NULL, approve_by timestamptz NOT NULL,
    approved_at timestamptz, expires_at timestamptz, verifier bytea, last_used_at timestamptz, revoked_at timestamptz,
    CHECK (approve_by=invited_at+interval '5 minutes'),
    CHECK ((verifier IS NULL)=(approved_at IS NULL) AND (approved_at IS NULL)=(expires_at IS NULL)
        AND octet_length(verifier)=32 AND approved_at<approve_by AND expires_at=approved_at+interval '24 hours')
);
CREATE INDEX device_owner_list ON relay_hosted.management_devices(installation_id,owner_id,invited_at DESC);
CREATE TABLE relay_hosted.device_changes (
    operation uuid PRIMARY KEY, device_id uuid NOT NULL REFERENCES relay_hosted.management_devices,
    action text NOT NULL CHECK(action IN ('invite','approve','revoke')), expected bigint NOT NULL,
    at timestamptz NOT NULL DEFAULT clock_timestamp(), UNIQUE(device_id,expected)
);
CREATE TRIGGER immutable_device_change BEFORE UPDATE OR DELETE ON relay_hosted.device_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.owner_devices(origin text,tag bytea) RETURNS jsonb
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
        'id',id,'label',label,'scope',scope,'revision',revision::text,'state',state,
        'expires_at_s',floor(extract(epoch FROM coalesce(expires_at,approve_by)))::bigint,
        'last_used_at_s',floor(extract(epoch FROM last_used_at))::bigint)
        ORDER BY (state IN ('pending','active')) DESC,invited_at DESC,id),'[]'::jsonb)) FROM rows);
END $$;

CREATE FUNCTION relay_hosted.change_device(origin text,tag bytea,op uuid,device uuid,rev bigint,act text,name text,proof bytea)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; actor uuid; ie bigint; ue bigint; signed_at timestamptz; stamp timestamptz;
    d relay_hosted.management_devices; prior relay_hosted.device_changes;
BEGIN
    IF op IS NULL OR device IS NULL OR rev IS NULL OR act IS NULL OR (op=device)<>(act='invite') OR act NOT IN ('invite','approve','revoke')
        OR (act='invite')<>(name IS NOT NULL) OR (act='approve')<>(proof IS NOT NULL)
        OR octet_length(proof)<>32 OR octet_length(name) NOT BETWEEN 1 AND 64 OR name<>btrim(name) OR name ~ '[[:cntrl:]]'
        THEN RETURN 'invalid'; END IF;
    SELECT id INTO iid FROM relay_hosted.installations WHERE public_origin=origin;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('relay-devices:'||iid::text,0));
    SELECT a.user_id,a.installation_epoch,a.user_epoch,s.created_at INTO actor,ie,ue,signed_at
        FROM relay_hosted.active_owner_credentials a JOIN relay_hosted.admin_sessions s
        ON (s.user_id,s.credential_id,s.installation_epoch,s.user_epoch)=(a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
        WHERE a.public_origin=origin AND s.verifier=tag AND s.expires_at>clock_timestamp()
        AND s.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF s,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    stamp:=clock_timestamp();
    IF signed_at<=stamp-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    SELECT * INTO d FROM relay_hosted.management_devices WHERE id=device AND installation_id=iid
        AND owner_id=actor AND installation_epoch=ie AND owner_epoch=ue FOR UPDATE;
    SELECT * INTO prior FROM relay_hosted.device_changes WHERE operation=op;
    IF FOUND THEN
        IF d.id IS NOT NULL AND (prior.device_id,prior.action,prior.expected)=(device,act,rev)
            AND (name IS NULL OR name=d.label) AND (proof IS NULL OR proof=d.verifier) THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF act='invite' THEN
        IF d.id IS NOT NULL OR rev<>0 OR op<>device OR EXISTS(SELECT FROM relay_hosted.management_devices WHERE id=device)
            THEN RETURN 'conflict'; END IF;
        IF (SELECT count(*) FROM relay_hosted.management_devices WHERE installation_id=iid AND owner_id=actor
            AND installation_epoch=ie AND owner_epoch=ue AND revoked_at IS NULL AND coalesce(expires_at,approve_by)>stamp)>=16
            THEN RETURN 'limit'; END IF;
        INSERT INTO relay_hosted.management_devices(id,installation_id,owner_id,installation_epoch,owner_epoch,label,scope,invited_at,approve_by)
            VALUES(device,iid,actor,ie,ue,name,'connections.read',stamp,stamp+interval '5 minutes');
    ELSE
        IF d.id IS NULL OR rev<>d.revision OR d.revoked_at IS NOT NULL OR coalesce(d.expires_at,d.approve_by)<=stamp
            THEN RETURN 'conflict'; END IF;
        IF act='approve' THEN
            IF d.approved_at IS NOT NULL THEN RETURN 'conflict'; END IF;
            UPDATE relay_hosted.management_devices SET approved_at=stamp,expires_at=stamp+interval '24 hours',
                verifier=proof,revision=revision+1 WHERE id=device;
        ELSE
            UPDATE relay_hosted.management_devices SET revoked_at=stamp,revision=revision+1 WHERE id=device;
        END IF;
    END IF;
    INSERT INTO relay_hosted.device_changes(operation,device_id,action,expected) VALUES(op,device,act,rev);
    RETURN 'applied';
END $$;

-- Only the two authenticated wrappers may call the common metadata projection.
CREATE FUNCTION relay_hosted.connection_page(iid uuid,after_id uuid) RETURNS jsonb
LANGUAGE sql SET search_path=pg_catalog AS $$
    WITH page AS MATERIALIZED (
        SELECT s.connection_id,s.generation IS NOT NULL AS stored FROM relay_hosted.connection_secret_slots s
        WHERE s.installation_id=iid AND s.purpose='provider' AND (after_id IS NULL OR s.connection_id>after_id)
        ORDER BY s.connection_id LIMIT 33
    ), visible AS (SELECT * FROM page ORDER BY connection_id LIMIT 32)
    SELECT jsonb_build_object('items',coalesce(jsonb_agg(jsonb_build_object(
        'id',v.connection_id,'label',(SELECT i.source_label FROM relay_hosted.text_inventory i
            WHERE i.source_id=v.connection_id ORDER BY i.checked_at DESC,i.id DESC LIMIT 1),
        'credential_state',CASE WHEN v.stored THEN 'stored' ELSE 'revoked' END,'proxy_bound',p.generation IS NOT NULL,
        'verified_models',ARRAY(SELECT DISTINCT i.model FROM relay_hosted.current_text_inventory i
            WHERE i.installation_id=iid AND i.source_id=v.connection_id ORDER BY i.model)
    ) ORDER BY v.connection_id),'[]'::jsonb),'next_after',CASE WHEN (SELECT count(*) FROM page)=33
        THEN (SELECT connection_id FROM visible ORDER BY connection_id DESC LIMIT 1) END)
    FROM visible v LEFT JOIN relay_hosted.connection_secret_slots p
        ON (p.installation_id,p.connection_id,p.purpose)=(iid,v.connection_id,'proxy')
$$;
CREATE OR REPLACE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT relay_hosted.connection_page(id,after_id) FROM relay_hosted.installations WHERE public_origin=origin);
END $$;
CREATE FUNCTION relay_hosted.device_connections(origin text,iid uuid,device uuid,proof bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE deadline timestamptz;
BEGIN
    UPDATE relay_hosted.management_devices d SET last_used_at=clock_timestamp()
    FROM relay_hosted.installations h WHERE h.public_origin=origin AND h.id=iid AND d.installation_id=h.id
        AND d.id=device AND d.verifier=proof AND d.scope='connections.read' AND d.revoked_at IS NULL AND d.expires_at>clock_timestamp()
        AND EXISTS(SELECT FROM relay_hosted.active_owner_credentials a WHERE a.public_origin=origin AND a.user_id=d.owner_id
            AND a.installation_epoch=d.installation_epoch AND a.user_epoch=d.owner_epoch)
        RETURNING d.expires_at INTO deadline;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN relay_hosted.connection_page(iid,after_id)||jsonb_build_object('installation_id',iid,'device_id',device,
        'scope','connections.read','expires_at_s',floor(extract(epoch FROM deadline))::bigint);
END $$;
REVOKE ALL ON FUNCTION relay_hosted.connection_page(uuid,uuid),relay_hosted.owner_devices(text,bytea),
    relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea),relay_hosted.device_connections(text,uuid,uuid,bytea,uuid) FROM PUBLIC;
