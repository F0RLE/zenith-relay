ALTER TABLE relay_hosted.customer_sessions
    ADD COLUMN id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    ADD COLUMN browser_instance uuid;

-- Session verifiers belong to one process key. Rows from older processes (or
-- before this migration) cannot appear in the current process's session list.
DROP FUNCTION relay_hosted.finish_customer_login(text,uuid,text,text,bytea,bytea,bigint,bigint,bytea);
CREATE FUNCTION relay_hosted.finish_customer_login(origin text, provider uuid, iss text, client text,
    config_tag bytea, subject_tag bytea, expected bigint, deadline_ms bigint, session_tag bytea, instance uuid)
RETURNS TABLE(result text, candidate_id uuid)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctl relay_hosted.service_control; epoch bigint; person uuid; space uuid; bound_space uuid;
    person_epoch bigint; scope_end timestamptz; stamp timestamptz;
BEGIN
    IF octet_length(config_tag)<>32 OR octet_length(subject_tag)<>32 OR octet_length(session_tag)<>32
        THEN RETURN; END IF;
    SELECT s.* INTO ctl FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
        WHERE i.singleton AND i.public_origin=origin AND i.setup_state='claimed' AND s.revision=expected
        FOR SHARE OF i,s;
    IF NOT FOUND OR NOT EXISTS (SELECT FROM relay_hosted.active_owner_credentials a WHERE a.public_origin=origin)
        THEN RETURN; END IF;
    IF ctl.mode IN ('setup','maintenance') THEN
        INSERT INTO relay_hosted.identity_providers (id,issuer,client_id,binding)
            VALUES (provider,iss,client,config_tag) ON CONFLICT DO NOTHING;
        PERFORM FROM relay_hosted.identity_providers WHERE id=provider AND issuer=iss AND client_id=client
            AND binding=config_tag FOR UPDATE;
        stamp := clock_timestamp();
        IF NOT FOUND OR stamp >= to_timestamp(deadline_ms/1000.0) THEN RETURN; END IF;
        DELETE FROM relay_hosted.identity_candidates WHERE provider_id=provider AND expires_at<=stamp;
        SELECT id INTO candidate_id FROM relay_hosted.identity_candidates WHERE provider_id=provider AND subject_digest=subject_tag;
        IF NOT FOUND THEN
            IF (SELECT count(*) FROM relay_hosted.identity_candidates WHERE provider_id=provider)>=64 THEN RETURN; END IF;
            INSERT INTO relay_hosted.identity_candidates(provider_id,subject_digest,expires_at)
                VALUES(provider,subject_tag,stamp+interval '10 minutes') RETURNING id INTO candidate_id;
        END IF;
        result := 'review'; RETURN NEXT; RETURN;
    END IF;
    SELECT q.workspace_id,r.expires_at INTO bound_space,scope_end FROM relay_hosted.qualification_subjects q
        JOIN relay_hosted.configuration_revisions r ON r.id=q.scope_id
        JOIN relay_hosted.identity_providers p ON p.id=r.provider_id
        WHERE ctl.mode='qualification' AND q.scope_id=ctl.qualification_scope_ref AND q.subject_digest=subject_tag
            AND p.id=provider AND p.issuer=iss AND p.client_id=client AND p.binding=config_tag FOR UPDATE OF q;
    IF NOT FOUND THEN RETURN; END IF;
    SELECT e.user_id INTO person FROM relay_hosted.external_identities e WHERE e.provider_id=provider AND e.subject_digest=subject_tag;
    IF NOT FOUND THEN
        stamp := clock_timestamp();
        IF bound_space IS NOT NULL OR stamp>=scope_end OR stamp >= to_timestamp(deadline_ms/1000.0) THEN RETURN; END IF;
        person := gen_random_uuid();
        INSERT INTO relay_hosted.users(id) VALUES(person);
        INSERT INTO relay_hosted.workspaces(owner_user_id) VALUES(person) RETURNING id INTO space;
        INSERT INTO relay_hosted.external_identities(provider_id,subject_digest,user_id) VALUES(provider,subject_tag,person);
        person_epoch := 1;
    ELSE
        SELECT w.id,u.authorization_epoch INTO space,person_epoch FROM relay_hosted.workspaces w
            JOIN relay_hosted.users u ON u.id=w.owner_user_id
            JOIN relay_hosted.external_identities e ON e.user_id=u.id AND e.provider_id=provider
            WHERE u.id=person AND u.disabled_at IS NULL AND e.subject_digest=subject_tag AND e.revoked_at IS NULL
              AND NOT EXISTS (SELECT FROM relay_hosted.admin_assignments a WHERE a.user_id=person) FOR UPDATE OF u,e;
        stamp := clock_timestamp();
        IF NOT FOUND OR (bound_space IS NOT NULL AND bound_space<>space)
            OR stamp>=scope_end OR stamp>=to_timestamp(deadline_ms/1000.0) THEN RETURN; END IF;
    END IF;
    UPDATE relay_hosted.qualification_subjects SET workspace_id=space
        WHERE scope_id=ctl.qualification_scope_ref AND subject_digest=subject_tag AND workspace_id IS NULL;
    SELECT i.authorization_epoch INTO STRICT epoch FROM relay_hosted.installations i WHERE i.id=ctl.installation_id;
    DELETE FROM relay_hosted.customer_sessions WHERE verifier IN
        (SELECT verifier FROM relay_hosted.customer_sessions WHERE user_id=person ORDER BY created_at DESC,verifier OFFSET 7);
    INSERT INTO relay_hosted.customer_sessions(verifier,scope_id,provider_id,subject_digest,user_id,workspace_id,
        installation_epoch,user_epoch,created_at,last_seen_at,expires_at,browser_instance)
        VALUES(session_tag,ctl.qualification_scope_ref,provider,subject_tag,person,space,epoch,person_epoch,
            stamp,stamp,least(stamp+interval '8 hours',scope_end),instance);
    result := 'signed_in'; RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.own_active_customer_sessions(actor relay_hosted.customer_sessions, stamp timestamptz)
RETURNS TABLE(id uuid,created_at timestamptz,last_seen_at timestamptz)
LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT s.id,s.created_at,s.last_seen_at FROM relay_hosted.customer_sessions s
    WHERE (s.user_id,s.workspace_id,s.scope_id,s.provider_id,s.subject_digest,s.installation_epoch,s.user_epoch,s.browser_instance)
        = ((actor).user_id,(actor).workspace_id,(actor).scope_id,(actor).provider_id,(actor).subject_digest,
            (actor).installation_epoch,(actor).user_epoch,(actor).browser_instance)
      AND s.expires_at>stamp AND s.last_seen_at>stamp-interval '30 minutes'
$$;

CREATE FUNCTION relay_hosted.read_customer_sessions(origin text, provider uuid, binding bytea, token bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor relay_hosted.customer_sessions;
BEGIN
    actor := relay_hosted.key_authority(origin,provider,binding,token,false);
    IF actor.browser_instance IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT CASE WHEN bool_or(p.current_session) THEN jsonb_agg(jsonb_build_object(
        'id',p.id,'current',p.current_session,'created_at_s',floor(extract(epoch FROM p.created_at))::bigint,
        'last_seen_at_s',floor(extract(epoch FROM p.last_seen_at))::bigint)
        ORDER BY p.current_session DESC,p.last_seen_at DESC,p.id) END
        FROM (SELECT a.*,a.id=actor.id AS current_session
              FROM relay_hosted.own_active_customer_sessions(actor,clock_timestamp()) a
              ORDER BY current_session DESC,a.last_seen_at DESC,a.id LIMIT 8) p);
END $$;

CREATE FUNCTION relay_hosted.revoke_customer_session(origin text, provider uuid, binding bytea, token bytea, target uuid)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor relay_hosted.customer_sessions;
BEGIN
    -- Lock the workspace before touching either session, so reciprocal
    -- revocations serialize and a revoked caller cannot continue a queued write.
    actor := relay_hosted.key_authority(origin,provider,binding,token,true);
    IF actor.browser_instance IS NULL THEN RETURN 'denied'; END IF;
    DELETE FROM relay_hosted.customer_sessions s
        USING relay_hosted.own_active_customer_sessions(actor,clock_timestamp()) a
        WHERE s.id=a.id AND a.id=target;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    RETURN CASE WHEN actor.id=target THEN 'current' ELSE 'other' END;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.finish_customer_login(text,uuid,text,text,bytea,bytea,bigint,bigint,bytea,uuid),
    relay_hosted.own_active_customer_sessions(relay_hosted.customer_sessions,timestamptz),
    relay_hosted.read_customer_sessions(text,uuid,bytea,bytea),
    relay_hosted.revoke_customer_session(text,uuid,bytea,bytea,uuid) FROM PUBLIC;
