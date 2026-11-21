ALTER TABLE relay_hosted.device_changes
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id);
ALTER TABLE relay_hosted.desktop_source_copies
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    ADD COLUMN device_revision bigint CHECK (device_revision BETWEEN 1 AND 3),
    ADD CHECK ((confirmed_by_passkey_id IS NULL)=(device_revision IS NULL));

DROP FUNCTION relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea);
DROP FUNCTION relay_hosted.review_source_copy(text,bytea,uuid,uuid,uuid,uuid,jsonb,bytea,bytea,boolean);

CREATE FUNCTION relay_hosted.change_device(origin text,tag bytea,op uuid,device uuid,rev bigint,act text,name text,proof bytea,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
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
        OR coalesce(d.expires_at,d.approve_by)<=stamp OR (act='approve' AND d.approved_at IS NOT NULL)
        THEN RETURN 'conflict';
    END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    SELECT outcome,confirmed.confirmer,verified_at INTO proof_result,confirmer,stamp
        FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,
            proof_backup,proof_eligible,proof_deadline_ms) confirmed;
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    IF act<>'invite' AND coalesce(d.expires_at,d.approve_by)<=stamp THEN RETURN 'conflict'; END IF;

    IF act='invite' THEN
        INSERT INTO relay_hosted.management_devices(id,installation_id,owner_id,installation_epoch,owner_epoch,label,scope,invited_at,approve_by)
            VALUES(device,iid,ss.user_id,ss.installation_epoch,ss.user_epoch,name,'connections.read',stamp,stamp+interval '5 minutes');
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

-- A missing revision is a read. An approval or receipt binds the exact review
-- and device revision; reading an existing operation never extends its grant.
CREATE FUNCTION relay_hosted.review_source_copy(origin text,tag bytea,iid uuid,device uuid,op uuid,source uuid,
    spec jsonb,proof bytea,payload bytea,device_rev bigint,proof_credential bytea,proof_revision bigint,
    proof_counter bigint,proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; stamp timestamptz; d relay_hosted.management_devices;
    c relay_hosted.desktop_source_copies; result text; proof_result text; confirmer uuid;
BEGIN
    IF origin NOT LIKE 'https://%' OR iid IS NULL OR device IS NULL OR op IS NULL OR source IS NULL
        OR spec IS NULL OR octet_length(spec::text)>2048
        OR NOT spec @> jsonb_build_object('origin',origin,'installation_id',iid,'device_id',device,'operation_id',op,'source_id',source)
        OR octet_length(proof) IS DISTINCT FROM 32 OR octet_length(payload) IS DISTINCT FROM 32
        OR device_rev NOT BETWEEN 1 AND 3 OR (device_rev IS NULL AND proof_credential IS NOT NULL)
        THEN RETURN '{"state":"invalid"}'; END IF;
    IF device_rev IS NULL THEN
        ss:=relay_hosted.owner_passkey_session(origin,tag);
    ELSE
        ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    END IF;
    IF ss.verifier IS NULL OR NOT EXISTS
        (SELECT FROM relay_hosted.installations WHERE id=iid AND public_origin=origin) THEN RETURN '{"state":"denied"}'; END IF;
    IF device_rev IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('relay-source-copy:'||iid::text,0));
        PERFORM FROM relay_hosted.management_devices WHERE id=device AND installation_id=iid FOR SHARE;
    END IF;
    SELECT * INTO d FROM relay_hosted.management_devices WHERE id=device AND installation_id=iid AND owner_id=ss.user_id
        AND installation_epoch=ss.installation_epoch AND owner_epoch=ss.user_epoch;
    IF d.id IS NULL THEN RETURN '{"state":"conflict"}'; END IF;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN '{"state":"denied"}'; END IF;
    SELECT * INTO c FROM relay_hosted.desktop_source_copies WHERE operation_id=op;
    IF FOUND THEN
        IF (c.installation_id,c.device_id,c.source_id,c.plan,c.verifier,c.commitment)<>(iid,device,source,spec,proof,payload)
            THEN RETURN '{"state":"conflict"}'; END IF;
        IF device_rev IS NOT NULL THEN
            RETURN CASE WHEN c.device_revision=device_rev THEN '{"state":"applied"}'::jsonb ELSE '{"state":"conflict"}'::jsonb END;
        END IF;
        result:=CASE WHEN c.committed_at IS NOT NULL THEN 'committed'
            WHEN c.device_revision IS DISTINCT FROM d.revision OR c.expires_at<=stamp OR d.scope<>'connections.read'
                OR d.revoked_at IS NOT NULL OR coalesce(d.expires_at<=stamp,true) THEN 'expired'
            ELSE 'approved' END;
    ELSE
        IF d.scope<>'connections.read' OR d.revoked_at IS NOT NULL OR coalesce(d.expires_at<=stamp,true)
            OR (device_rev IS NOT NULL AND device_rev<>d.revision)
            OR EXISTS(SELECT FROM relay_hosted.connection_secret_slots WHERE installation_id=iid AND connection_id=source)
            OR EXISTS(SELECT FROM relay_hosted.static_source_provisions r WHERE r.installation_id=iid AND r.operation_id=op)
            OR EXISTS(SELECT FROM relay_hosted.desktop_source_copies f WHERE f.installation_id=iid AND f.source_id=source
                AND f.device_revision IS NOT NULL AND f.expires_at>stamp AND f.committed_at IS NULL)
            THEN RETURN '{"state":"conflict"}'; END IF;
        IF device_rev IS NULL THEN
            result:='awaiting_approval';
        ELSE
            IF (SELECT count(*) FROM relay_hosted.desktop_source_copies f WHERE f.installation_id=iid
                AND f.device_revision IS NOT NULL AND f.expires_at>stamp AND f.committed_at IS NULL)>=32
                THEN RETURN '{"state":"limit"}'; END IF;
            IF proof_credential IS NULL THEN RETURN '{"state":"fresh_required"}'; END IF;
            SELECT outcome,confirmed.confirmer,verified_at INTO proof_result,confirmer,stamp
                FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,
                    proof_backup,proof_eligible,proof_deadline_ms) confirmed;
            IF proof_result<>'verified' THEN RETURN jsonb_build_object('state',proof_result); END IF;
            IF d.expires_at<=stamp THEN RETURN '{"state":"conflict"}'; END IF;
            INSERT INTO relay_hosted.desktop_source_copies(operation_id,installation_id,device_id,source_id,plan,verifier,commitment,
                approved_at,expires_at,device_revision,confirmed_by_passkey_id)
                VALUES(op,iid,device,source,spec,proof,payload,stamp,stamp+interval '10 minutes',device_rev,confirmer);
            RETURN '{"state":"applied"}';
        END IF;
    END IF;
    RETURN jsonb_build_object('state',result,'device_label',d.label,'device_revision',d.revision::text,'plan',spec);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.source_copy_access(origin text,iid uuid,device uuid,op uuid,proof bytea,applying boolean)
RETURNS TABLE(state text,plan jsonb,payload_tag bytea)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE c relay_hosted.desktop_source_copies; d relay_hosted.management_devices;
BEGIN
    IF NOT EXISTS (SELECT FROM relay_hosted.installations h WHERE h.id=iid AND h.public_origin=origin)
        THEN RETURN; END IF;
    SELECT * INTO c FROM relay_hosted.desktop_source_copies k WHERE k.operation_id=op;
    IF c.operation_id IS NOT NULL AND (c.installation_id,c.device_id,c.verifier)<>(iid,device,proof)
        THEN RETURN; END IF;
    IF applying AND c.device_revision IS NOT NULL AND c.committed_at IS NULL THEN
        -- Match web writes: authority, device, then copy row, then the
        -- worker's vault lock. Do not lock passkeys: owner key revocation
        -- already takes the installation/service fence exclusively.
        SELECT m.* INTO d FROM relay_hosted.installations h
            JOIN relay_hosted.service_control s ON s.installation_id=h.id
            JOIN relay_hosted.users u ON u.id=h.owner_user_id
            JOIN relay_hosted.admin_assignments a ON a.user_id=u.id
            JOIN relay_hosted.management_devices m ON
                (m.installation_id,m.owner_id,m.installation_epoch,m.owner_epoch)
                =(h.id,u.id,h.authorization_epoch,u.authorization_epoch)
            WHERE h.id=iid AND h.public_origin=origin AND h.singleton AND h.setup_state='claimed'
                AND s.mode IN ('setup','qualification','maintenance') AND s.restore_operation IS NULL
                AND a.role='owner' AND a.revoked_at IS NULL AND u.disabled_at IS NULL AND m.id=device
                AND EXISTS (SELECT FROM relay_hosted.admin_credentials k WHERE k.user_id=u.id AND k.revoked_at IS NULL)
            FOR SHARE OF h,s,u,a,m;
        SELECT * INTO c FROM relay_hosted.desktop_source_copies k WHERE k.operation_id=op FOR UPDATE;
    ELSIF c.committed_at IS NULL AND (c.operation_id IS NULL OR c.device_revision IS NOT NULL) THEN
        SELECT m.* INTO d FROM relay_hosted.management_devices m
            WHERE m.id=device AND m.installation_id=iid AND EXISTS
                (SELECT FROM relay_hosted.active_owner_credentials a WHERE
                    (a.public_origin,a.user_id,a.installation_epoch,a.user_epoch)
                    =(origin,m.owner_id,m.installation_epoch,m.owner_epoch));
    END IF;
    IF c.committed_at IS NOT NULL THEN
        IF c.committed_at>clock_timestamp()-interval '7 days' THEN
            RETURN QUERY SELECT 'committed'::text,NULL::jsonb,NULL::bytea;
        END IF;
    ELSIF d.id IS NULL OR d.revoked_at IS NOT NULL OR d.scope<>'connections.read'
        OR coalesce(d.expires_at<=clock_timestamp(),true) THEN
        IF c.operation_id IS NOT NULL THEN RETURN QUERY SELECT 'expired'::text,NULL::jsonb,NULL::bytea; END IF;
    ELSIF c.operation_id IS NULL THEN
        RETURN QUERY SELECT 'awaiting_approval'::text,NULL::jsonb,NULL::bytea;
    ELSIF c.device_revision IS DISTINCT FROM d.revision OR c.expires_at<=clock_timestamp() THEN
        RETURN QUERY SELECT 'expired'::text,NULL::jsonb,NULL::bytea;
    ELSE
        RETURN QUERY SELECT 'approved'::text,c.plan,c.commitment;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.finish_source_copy(origin text,iid uuid,device uuid,op uuid,proof bytea)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz;
BEGIN
    PERFORM 1 FROM relay_hosted.source_copy_access(origin,iid,device,op,proof,true) a WHERE a.state='approved';
    IF NOT FOUND THEN RETURN false; END IF;
    stamp:=clock_timestamp();
    UPDATE relay_hosted.desktop_source_copies c SET committed_at=stamp
        WHERE c.operation_id=op AND c.expires_at>stamp AND c.committed_at IS NULL
        AND EXISTS(SELECT FROM relay_hosted.management_devices d WHERE d.id=device AND d.expires_at>stamp)
        AND EXISTS(SELECT FROM relay_hosted.static_source_provisions p WHERE
            (p.installation_id,p.operation_id,p.source_id,p.proxied)=(iid,op,c.source_id,c.plan->'proxy'<>'null'::jsonb));
    RETURN FOUND;
END $$;

REVOKE ALL ON FUNCTION
    relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.review_source_copy(text,bytea,uuid,uuid,uuid,uuid,jsonb,bytea,bytea,bigint,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
