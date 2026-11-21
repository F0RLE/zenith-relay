CREATE TABLE relay_hosted.identity_providers (
    id uuid PRIMARY KEY,
    issuer text COLLATE "C" NOT NULL CHECK (octet_length(issuer) BETWEEN 1 AND 2048),
    client_id text COLLATE "C" NOT NULL CHECK (octet_length(client_id) BETWEEN 1 AND 256),
    binding bytea NOT NULL CHECK (octet_length(binding)=32),
    UNIQUE (issuer, client_id)
);
CREATE TABLE relay_hosted.identity_candidates (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id uuid NOT NULL REFERENCES relay_hosted.identity_providers,
    subject_digest bytea NOT NULL CHECK (octet_length(subject_digest)=32),
    expires_at timestamptz NOT NULL,
    UNIQUE (provider_id, subject_digest)
);
CREATE INDEX candidate_expiry ON relay_hosted.identity_candidates (provider_id, expires_at);

CREATE FUNCTION relay_hosted.customer_binding_matches(provider uuid, iss text, client text, config_tag bytea)
RETURNS boolean LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT NOT EXISTS (SELECT FROM relay_hosted.identity_providers p
        WHERE (p.id=provider OR (p.issuer=iss AND p.client_id=client))
        AND (p.id<>provider OR p.issuer<>iss OR p.client_id<>client OR p.binding<>config_tag))
$$;
REVOKE ALL ON FUNCTION relay_hosted.customer_binding_matches(uuid,text,text,bytea) FROM PUBLIC;

-- Immutable owner operations also serve as the revisioned, private scope and audit.
CREATE TABLE relay_hosted.configuration_revisions (
    id uuid PRIMARY KEY,
    revision bigint NOT NULL UNIQUE CHECK (revision > 1),
    expected_revision bigint NOT NULL CHECK (revision=expected_revision+1),
    mode text NOT NULL CHECK (mode IN ('qualification','maintenance')),
    reason_code text NOT NULL CHECK ((mode='qualification' AND reason_code='verify_customer_access')
                                 OR (mode='maintenance' AND reason_code='pause_customer_access')),
    actor_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    provider_id uuid NOT NULL REFERENCES relay_hosted.identity_providers,
    candidate_ids uuid[] NOT NULL,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at >= created_at),
    CHECK ((mode='qualification' AND cardinality(candidate_ids) BETWEEN 1 AND 16)
        OR (mode='maintenance' AND cardinality(candidate_ids)=0))
);
ALTER TABLE relay_hosted.service_control DROP CONSTRAINT service_control_mode_check,
    ADD COLUMN qualification_scope_ref uuid REFERENCES relay_hosted.configuration_revisions,
    ADD CONSTRAINT private_admission_mode CHECK (
        (mode IN ('setup','maintenance') AND qualification_scope_ref IS NULL)
        OR (mode='qualification' AND qualification_scope_ref IS NOT NULL));
CREATE TABLE relay_hosted.qualification_subjects (
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    subject_digest bytea NOT NULL CHECK (octet_length(subject_digest)=32),
    workspace_id uuid REFERENCES relay_hosted.workspaces,
    PRIMARY KEY (scope_id, subject_digest),
    UNIQUE (scope_id, workspace_id)
);
CREATE TABLE relay_hosted.external_identities (
    provider_id uuid NOT NULL REFERENCES relay_hosted.identity_providers,
    subject_digest bytea NOT NULL CHECK (octet_length(subject_digest)=32),
    user_id uuid NOT NULL REFERENCES relay_hosted.users,
    linked_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    PRIMARY KEY (provider_id, subject_digest),
    UNIQUE (provider_id, user_id),
    UNIQUE (provider_id, subject_digest, user_id)
);
ALTER TABLE relay_hosted.workspaces ADD CONSTRAINT workspace_person UNIQUE (id, owner_user_id);
CREATE TABLE relay_hosted.customer_sessions (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier)=32),
    scope_id uuid NOT NULL,
    provider_id uuid NOT NULL,
    subject_digest bytea NOT NULL,
    user_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    installation_epoch bigint NOT NULL,
    user_epoch bigint NOT NULL,
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > created_at),
    FOREIGN KEY (provider_id, subject_digest, user_id) REFERENCES relay_hosted.external_identities(provider_id,subject_digest,user_id),
    FOREIGN KEY (workspace_id, user_id) REFERENCES relay_hosted.workspaces(id,owner_user_id),
    FOREIGN KEY (scope_id, subject_digest) REFERENCES relay_hosted.qualification_subjects(scope_id,subject_digest)
);
CREATE INDEX customer_sessions_recent ON relay_hosted.customer_sessions (user_id,created_at DESC,verifier);

CREATE OR REPLACE VIEW relay_hosted.closed_installation AS
    SELECT i.public_origin FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id=i.id
    WHERE i.singleton AND s.mode IN ('setup','qualification','maintenance');
CREATE OR REPLACE VIEW relay_hosted.active_owner_credentials AS
    SELECT i.public_origin,i.authorization_epoch AS installation_epoch,u.id AS user_id,
        u.authorization_epoch AS user_epoch,c.credential_id,c.revision,c.passkey,s.mode
    FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id=i.id
    JOIN relay_hosted.users u ON u.id=i.owner_user_id
    JOIN relay_hosted.admin_assignments a ON a.user_id=u.id
    JOIN relay_hosted.admin_credentials c ON c.user_id=u.id
    WHERE i.singleton AND i.setup_state='claimed' AND s.mode IN ('setup','qualification','maintenance')
        AND a.role='owner' AND a.revoked_at IS NULL AND c.revoked_at IS NULL AND u.disabled_at IS NULL;
CREATE OR REPLACE FUNCTION relay_hosted.owner_login_context(text)
RETURNS TABLE(installation_epoch bigint,user_epoch bigint,deadline_ms bigint)
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT i.authorization_epoch,coalesce(u.authorization_epoch,0),
        floor(extract(epoch FROM clock_timestamp())*1000)::bigint+120000
    FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
    LEFT JOIN relay_hosted.users u ON u.id=i.owner_user_id
    WHERE i.singleton AND i.public_origin=$1 AND s.mode IN ('setup','qualification','maintenance')
$$;

CREATE FUNCTION relay_hosted.customer_login_context(origin text)
RETURNS TABLE(revision bigint,deadline_ms bigint)
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT s.revision,floor(extract(epoch FROM clock_timestamp())*1000)::bigint+180000
    FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
    WHERE i.singleton AND i.public_origin=origin AND i.setup_state='claimed'
        AND EXISTS (SELECT FROM relay_hosted.active_owner_credentials a WHERE a.public_origin=origin)
$$;

-- Only called after code exchange and ID-token verification. Serialize on the scope
-- member, not on the whole installation, when creating a customer and its wallet.
CREATE FUNCTION relay_hosted.finish_customer_login(origin text, provider uuid, iss text, client text,
    config_tag bytea, subject_tag bytea, expected bigint, deadline_ms bigint, session_tag bytea)
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
    INSERT INTO relay_hosted.customer_sessions VALUES (session_tag,ctl.qualification_scope_ref,provider,subject_tag,
        person,space,epoch,person_epoch,stamp,stamp,least(stamp+interval '8 hours',scope_end));
    result := 'signed_in'; RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.customer_account(origin text, provider uuid, config_tag bytea, session_tag bytea)
RETURNS TABLE(workspace_id uuid,available bigint,reserved bigint)
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    UPDATE relay_hosted.customer_sessions ss SET last_seen_at=clock_timestamp()
    FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
    JOIN relay_hosted.configuration_revisions r ON r.id=s.qualification_scope_ref
    JOIN relay_hosted.identity_providers p ON p.id=r.provider_id
    JOIN relay_hosted.qualification_subjects q ON q.scope_id=r.id
    JOIN relay_hosted.external_identities e ON e.provider_id=p.id AND e.subject_digest=q.subject_digest
    JOIN relay_hosted.users u ON u.id=e.user_id JOIN relay_hosted.wallets w ON w.workspace_id=q.workspace_id
    WHERE ss.verifier=session_tag AND ss.scope_id=q.scope_id AND ss.workspace_id=w.workspace_id AND ss.user_id=u.id
        AND ss.provider_id=p.id AND ss.subject_digest=q.subject_digest AND p.id=provider AND p.binding=config_tag
        AND i.public_origin=origin AND i.authorization_epoch=ss.installation_epoch AND s.mode='qualification'
        AND u.disabled_at IS NULL AND e.revoked_at IS NULL AND u.authorization_epoch=ss.user_epoch
        AND ss.expires_at>clock_timestamp() AND r.expires_at>clock_timestamp() AND ss.last_seen_at>clock_timestamp()-interval '30 minutes'
    RETURNING ss.workspace_id,w.available_micro_usd,w.reserved_micro_usd
$$;
CREATE FUNCTION relay_hosted.end_customer_session(bytea)
RETURNS boolean LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH gone AS (DELETE FROM relay_hosted.customer_sessions WHERE verifier=$1 RETURNING 1) SELECT EXISTS(SELECT FROM gone)
$$;
REVOKE ALL ON FUNCTION relay_hosted.customer_login_context(text),
    relay_hosted.finish_customer_login(text,uuid,text,text,bytea,bytea,bigint,bigint,bytea),
    relay_hosted.customer_account(text,uuid,bytea,bytea),relay_hosted.end_customer_session(bytea) FROM PUBLIC;
