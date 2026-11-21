ALTER TABLE relay_hosted.management_devices ADD COLUMN manage_pool boolean NOT NULL DEFAULT false;

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
        'id',id,'label',label,'scope',scope,'read_scopes',read_scopes,'manage_pool',manage_pool,'revision',revision::text,'state',state,
        'expires_at_s',floor(extract(epoch FROM coalesce(expires_at,approve_by)))::bigint,
        'last_used_at_s',floor(extract(epoch FROM last_used_at))::bigint)
        ORDER BY (state IN ('pending','active')) DESC,invited_at DESC,id),'[]'::jsonb)) FROM rows);
END $$;

DROP FUNCTION relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea,text[],bytea,bigint,bigint,boolean,boolean,bigint);

CREATE FUNCTION relay_hosted.change_device(origin text,tag bytea,op uuid,device uuid,rev bigint,act text,name text,proof bytea,
    scopes text[],pool_grant boolean,proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,
    proof_eligible boolean,proof_deadline_ms bigint)
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
        OR pool_grant IS NULL OR (act='revoke' AND (scopes<>'{}' OR pool_grant))
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
            AND (act='revoke' OR (d.read_scopes,d.manage_pool)=(scopes,pool_grant))
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
        OR (act='approve' AND (d.approved_at IS NOT NULL OR (d.read_scopes,d.manage_pool)<>(scopes,pool_grant)))
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
            read_scopes,manage_pool,invited_at,approve_by)
            VALUES(device,iid,ss.user_id,ss.installation_epoch,ss.user_epoch,name,'connections.read',scopes,pool_grant,stamp,
                stamp+interval '5 minutes');
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

CREATE TABLE relay_hosted.desktop_pool_reviews (
    operation_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    device_id uuid NOT NULL REFERENCES relay_hosted.management_devices,
    device_revision bigint NOT NULL,
    source_id uuid NOT NULL,
    plan jsonb NOT NULL,
    expires_at timestamptz NOT NULL,
    applied_at timestamptz,
    confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    CHECK ((applied_at IS NULL)=(confirmed_by_passkey_id IS NULL)),
    CHECK (applied_at<expires_at)
);
CREATE UNIQUE INDEX desktop_pool_pending ON relay_hosted.desktop_pool_reviews(device_id) WHERE applied_at IS NULL;
CREATE TRIGGER sealed_desktop_pool_review BEFORE UPDATE OR DELETE ON relay_hosted.desktop_pool_reviews
    FOR EACH ROW WHEN (OLD.applied_at IS NOT NULL) EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

-- Take the newest binding per exact model; an older check cannot mask its
-- expiry or fence. Parking reviews the complete admitted set.
-- Use the existing inventory epoch and credential revisions, not a new routing state.
CREATE FUNCTION relay_hosted.desktop_pool_plan(iid uuid,source uuid)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    WITH context AS (
        SELECT s.qualification_scope_ref AS scope_id, coalesce(v.revision,0) AS revision, coalesce(v.inventory_epoch,0) AS epoch,
            relay_hosted.source_in_pool(s.qualification_scope_ref,source,coalesce(v.inventory_epoch,0)) AS member,
            cs.revision AS provider_revision, p.revision AS proxy_revision
        FROM relay_hosted.service_control s JOIN relay_hosted.connection_secret_slots cs
            ON (cs.installation_id,cs.connection_id,cs.purpose)=(s.installation_id,source,'provider') AND cs.generation IS NOT NULL
        LEFT JOIN relay_hosted.connection_secret_slots p ON (p.installation_id,p.connection_id,p.purpose)=(iid,source,'proxy')
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(iid,source)
        WHERE s.installation_id=iid AND s.qualification_scope_ref IS NOT NULL AND v.action IS DISTINCT FROM 'trash'
            AND (NOT coalesce(v.paused,false) OR relay_hosted.source_in_pool(s.qualification_scope_ref,source,coalesce(v.inventory_epoch,0)))
            AND coalesce(v.revision,0)<9223372036854775807
    )
    SELECT jsonb_build_object('action',CASE WHEN c.member THEN 'exclude' ELSE 'include' END,
        'scope_id',c.scope_id,'source_revision',c.revision::text,'provider_revision',c.provider_revision,
        'proxy_revision',c.proxy_revision,'label',bindings.label,'items',bindings.items)
    FROM context c CROSS JOIN LATERAL (
        SELECT jsonb_agg(jsonb_build_object('id',i.id,'revision',i.revision::text,'model',i.model) ORDER BY i.model COLLATE "C",i.id) AS items,
            (array_agg(i.source_label ORDER BY i.checked_at DESC,i.id))[1] AS label
        FROM (
            SELECT DISTINCT ON (i.model,CASE WHEN c.member THEN i.id END) i.id,i.model,r.revision,i.checked_at,i.source_label
            FROM relay_hosted.text_inventory i JOIN relay_hosted.text_model_rules r USING(id)
            WHERE (i.scope_id,i.source_id,i.inventory_epoch)=(c.scope_id,source,c.epoch)
                AND r.admitted=c.member
            ORDER BY i.model,CASE WHEN c.member THEN i.id END,i.checked_at DESC,i.id
        ) i
        WHERE c.member OR (i.revision<9223372036854775807 AND NOT relay_hosted.text_inventory_fenced(i.id)
            AND EXISTS(SELECT FROM relay_hosted.current_text_inventory current WHERE current.id=i.id))
    ) bindings WHERE bindings.items IS NOT NULL
$$;

CREATE FUNCTION relay_hosted.desktop_pool_revision(plan jsonb)
RETURNS text LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT encode(sha256(convert_to(plan::text,'UTF8')),'hex')
$$;

CREATE FUNCTION relay_hosted.device_pool_connections(origin text,iid uuid,device uuid,proof bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE page jsonb; permitted boolean;
BEGIN
    page:=relay_hosted.device_connections(origin,iid,device,proof,after_id);
    IF page IS NULL THEN RETURN NULL; END IF;
    SELECT d.manage_pool INTO STRICT permitted FROM relay_hosted.management_devices d WHERE d.id=device;
    RETURN jsonb_set(page,'{items}',(SELECT coalesce(jsonb_agg(item||jsonb_build_object('pool',
        CASE WHEN sv.view IS NOT NULL THEN sv.view-'revision'||jsonb_build_object(
            'action',p.plan->'action','revision',relay_hosted.desktop_pool_revision(p.plan)) END)
        ORDER BY n),'[]'::jsonb) FROM jsonb_array_elements(page->'items') WITH ORDINALITY AS rows(item,n)
        CROSS JOIN LATERAL (SELECT relay_hosted.source_serving_view(iid,(item->>'id')::uuid) AS view) sv
        CROSS JOIN LATERAL (SELECT CASE WHEN permitted THEN relay_hosted.desktop_pool_plan(iid,(item->>'id')::uuid) END AS plan) p))
        || jsonb_build_object('pool_access',permitted,
            'pool_review',(SELECT jsonb_build_object('operation_id',r.operation_id,
                'source_id',r.source_id,'action',r.plan->'action',
                'expires_at_s',floor(extract(epoch FROM r.expires_at))::bigint)
                FROM relay_hosted.desktop_pool_reviews r JOIN relay_hosted.management_devices d ON d.id=r.device_id
                WHERE permitted AND r.device_id=device AND r.device_revision=d.revision AND r.expires_at>statement_timestamp()
                    AND r.applied_at IS NULL));
END $$;

CREATE FUNCTION relay_hosted.prepare_desktop_pool_review(origin text,iid uuid,device uuid,proof bytea,source uuid,
    action text,expected text)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE d relay_hosted.management_devices; draft jsonb; prior relay_hosted.desktop_pool_reviews; stamp timestamptz;
BEGIN
    IF action NOT IN ('include','exclude') OR expected COLLATE "C" !~ '^[0-9a-f]{64}$'
        OR source='00000000-0000-0000-0000-000000000000' THEN RETURN '{"error":"invalid"}'; END IF;
    IF relay_hosted.device_read_context(origin,iid,device,proof,NULL) IS NULL THEN RETURN NULL; END IF;
    SELECT * INTO d FROM relay_hosted.management_devices m WHERE m.id=device AND m.manage_pool
        AND m.revoked_at IS NULL AND m.expires_at>clock_timestamp() AND m.verifier=proof FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN NULL; END IF;
    draft:=relay_hosted.desktop_pool_plan(iid,source);
    IF draft->>'action' IS DISTINCT FROM action OR relay_hosted.desktop_pool_revision(draft) IS DISTINCT FROM expected
        THEN RETURN '{"error":"conflict"}'; END IF;
    stamp:=clock_timestamp();
    IF d.expires_at<=stamp THEN RETURN NULL; END IF;
    SELECT * INTO prior FROM relay_hosted.desktop_pool_reviews r WHERE r.device_id=device AND r.applied_at IS NULL;
    IF (prior.source_id,prior.plan,prior.device_revision) IS DISTINCT FROM (source,draft,d.revision) OR prior.expires_at<=stamp THEN
        DELETE FROM relay_hosted.desktop_pool_reviews r WHERE r.device_id=device AND r.applied_at IS NULL;
        INSERT INTO relay_hosted.desktop_pool_reviews(device_id,device_revision,source_id,plan,expires_at)
            VALUES(device,d.revision,source,draft,least(stamp+interval '5 minutes',d.expires_at)) RETURNING * INTO prior;
    END IF;
    RETURN jsonb_build_object('installation_id',iid,'device_id',device,'expires_at_s',floor(extract(epoch FROM d.expires_at))::bigint,
        'result',jsonb_build_object('operation_id',prior.operation_id,'source_id',source,'action',action,
            'expires_at_s',floor(extract(epoch FROM prior.expires_at))::bigint));
END $$;

CREATE FUNCTION relay_hosted.owner_desktop_pool_review(origin text,tag bytea,op uuid)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    RETURN coalesce((SELECT CASE WHEN d.revoked_at IS NOT NULL OR d.expires_at<=statement_timestamp()
        OR d.revision<>r.device_revision OR (r.applied_at IS NULL AND (r.expires_at<=statement_timestamp()
            OR r.plan IS DISTINCT FROM relay_hosted.desktop_pool_plan(h.id,r.source_id)))
        THEN '{"error":"conflict"}'::jsonb ELSE jsonb_build_object(
            'operation_id',op,'action',r.plan->'action','source_id',r.source_id,'source_label',r.plan->'label',
            'device_label',d.label,'models',ARRAY(SELECT DISTINCT (m->>'model') COLLATE "C"
                FROM jsonb_array_elements(r.plan->'items') m ORDER BY 1),
            'applied',r.applied_at IS NOT NULL) END
        FROM relay_hosted.desktop_pool_reviews r JOIN relay_hosted.management_devices d ON d.id=r.device_id
        JOIN relay_hosted.installations h ON h.id=d.installation_id AND h.owner_user_id=d.owner_id
        JOIN relay_hosted.users u ON u.id=d.owner_id
        WHERE r.operation_id=op AND h.public_origin=origin AND d.manage_pool
            AND (d.installation_epoch,d.owner_epoch)=(h.authorization_epoch,u.authorization_epoch)), '{"error":"missing"}');
END $$;

CREATE FUNCTION relay_hosted.apply_desktop_pool_review(origin text,tag bytea,op uuid,proof_credential bytea,proof_revision bigint,
    proof_counter bigint,proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; d relay_hosted.management_devices; r relay_hosted.desktop_pool_reviews;
    stamp timestamptz; confirmed uuid; outcome_code text; scope uuid; model_id text; deadline bigint;
BEGIN
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT m.* INTO d FROM relay_hosted.management_devices m JOIN relay_hosted.desktop_pool_reviews v ON v.device_id=m.id
        JOIN relay_hosted.installations h ON h.id=m.installation_id AND h.public_origin=origin
        WHERE v.operation_id=op AND m.owner_id=ss.user_id AND m.installation_epoch=ss.installation_epoch
            AND m.owner_epoch=ss.user_epoch AND m.manage_pool FOR SHARE OF m;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO r FROM relay_hosted.desktop_pool_reviews v WHERE v.operation_id=op FOR UPDATE;
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN 'denied'; END IF;
    IF r.operation_id IS NULL OR d.revoked_at IS NOT NULL OR d.revision<>r.device_revision
        OR d.expires_at<=clock_timestamp() THEN RETURN 'conflict'; END IF;
    IF r.applied_at IS NOT NULL THEN RETURN 'applied'; END IF;
    IF r.expires_at<=clock_timestamp() THEN RETURN 'conflict'; END IF;

    IF proof_credential IS NOT NULL THEN
        IF proof_deadline_ms IS NULL THEN RETURN 'expired'; END IF;
        IF r.plan->>'action'='exclude' THEN
            PERFORM pg_advisory_xact_lock(hashtextextended('source-serving-operation:'||op::text,0));
        END IF;
        IF relay_hosted.lock_supply_signer(ss.user_id,proof_credential) IS DISTINCT FROM true THEN RETURN 'passkey_changed'; END IF;
        -- Same signer/vault/publication ordering as web writes. Inclusion takes its
        -- rule rows first: a dispatch can hold rule rows while waiting for the source.
        PERFORM pg_advisory_xact_lock_shared(98653249,1);
        SELECT qualification_scope_ref INTO STRICT scope FROM relay_hosted.service_control WHERE installation_id=d.installation_id;
        FOR model_id IN SELECT DISTINCT i.model FROM relay_hosted.text_inventory i
            WHERE (i.scope_id,i.source_id)=(scope,r.source_id) ORDER BY i.model
        LOOP
            PERFORM pg_advisory_xact_lock(hashtextextended('text-publication:'||scope::text||':'||model_id,0));
        END LOOP;
        IF r.plan->>'action'='include' THEN
            PERFORM FROM relay_hosted.text_model_rules m JOIN jsonb_array_elements(r.plan->'items') i ON m.id=(i->>'id')::uuid
                ORDER BY m.model,m.id FOR NO KEY UPDATE OF m;
        END IF;
        PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||r.source_id::text,0));
    END IF;
    IF r.expires_at<=clock_timestamp() OR r.plan IS DISTINCT FROM relay_hosted.desktop_pool_plan(d.installation_id,r.source_id)
        THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    deadline:=least(proof_deadline_ms,floor(extract(epoch FROM r.expires_at)*1000)::bigint);
    IF r.plan->>'action'='exclude' THEN
        outcome_code:=relay_hosted.change_source_serving(origin,tag,r.source_id,op,(r.plan->>'source_revision')::bigint,
            'park',proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,deadline);
        IF outcome_code<>'applied' THEN RETURN outcome_code; END IF;
        SELECT c.confirmed_by_passkey_id,c.changed_at INTO STRICT confirmed,stamp
            FROM relay_hosted.source_serving_changes c WHERE (c.installation_id,c.operation_id)=(d.installation_id,op);
    ELSE
        SELECT outcome,confirmer,verified_at INTO outcome_code,confirmed,stamp
            FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,deadline);
        IF outcome_code<>'verified' THEN RETURN outcome_code; END IF;
        IF r.plan IS DISTINCT FROM relay_hosted.desktop_pool_plan(d.installation_id,r.source_id) THEN RETURN 'conflict'; END IF;
        -- Admission grants neither purchase rates nor a published offer. Preserve
        -- any existing per-member model policy.
        UPDATE relay_hosted.text_model_rules m SET admitted=true,revision=m.revision+1
            FROM jsonb_array_elements(r.plan->'items') i WHERE m.id=(i->>'id')::uuid;
    END IF;
    UPDATE relay_hosted.desktop_pool_reviews SET applied_at=stamp,confirmed_by_passkey_id=confirmed WHERE operation_id=op;
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION
    relay_hosted.change_device(text,bytea,uuid,uuid,bigint,text,text,bytea,text[],boolean,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.desktop_pool_plan(uuid,uuid),
    relay_hosted.desktop_pool_revision(jsonb),
    relay_hosted.device_pool_connections(text,uuid,uuid,bytea,uuid),
    relay_hosted.prepare_desktop_pool_review(text,uuid,uuid,bytea,uuid,text,text),
    relay_hosted.owner_desktop_pool_review(text,bytea,uuid),
    relay_hosted.apply_desktop_pool_review(text,bytea,uuid,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
