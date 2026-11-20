ALTER TABLE relay_hosted.users ADD COLUMN disabled_at timestamptz;
ALTER TABLE relay_hosted.admin_assignments ADD COLUMN revoked_at timestamptz;
ALTER TABLE relay_hosted.admin_credentials
    ADD COLUMN revoked_at timestamptz,
    ADD COLUMN revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0);

CREATE TABLE relay_hosted.admin_sessions (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier) = 32),
    user_id uuid NOT NULL REFERENCES relay_hosted.users,
    credential_id bytea NOT NULL REFERENCES relay_hosted.admin_credentials,
    installation_epoch bigint NOT NULL,
    user_epoch bigint NOT NULL,
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at > created_at)
);

-- Private even to the web role; functions below expose individual checks only.
CREATE VIEW relay_hosted.active_owner_credentials AS
    SELECT i.public_origin, i.authorization_epoch AS installation_epoch,
        u.id AS user_id, u.authorization_epoch AS user_epoch, c.credential_id,
        c.revision, c.passkey, s.mode
    FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id = i.id
    JOIN relay_hosted.users u ON u.id = i.owner_user_id
    JOIN relay_hosted.admin_assignments a ON a.user_id = u.id
    JOIN relay_hosted.admin_credentials c ON c.user_id = u.id
    WHERE i.singleton AND i.setup_state = 'claimed' AND s.mode = 'setup'
        AND a.role = 'owner' AND a.revoked_at IS NULL AND c.revoked_at IS NULL AND u.disabled_at IS NULL;

CREATE FUNCTION relay_hosted.owner_login_context(text)
RETURNS TABLE(installation_epoch bigint, user_epoch bigint, deadline_ms bigint)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT i.authorization_epoch, coalesce(u.authorization_epoch, 0),
        floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint + 120000
    FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
    LEFT JOIN relay_hosted.users u ON u.id=i.owner_user_id
    WHERE i.singleton AND i.public_origin=$1 AND s.mode='setup'
$$;

CREATE FUNCTION relay_hosted.owner_verifier(text, uuid, bytea, bigint, bigint)
RETURNS TABLE(revision bigint, passkey jsonb)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
    SELECT a.revision, a.passkey FROM relay_hosted.active_owner_credentials a
    WHERE a.public_origin=$1 AND a.user_id=$2 AND a.credential_id=$3
        AND a.installation_epoch=$4 AND a.user_epoch=$5
$$;

CREATE FUNCTION relay_hosted.issue_admin_session(text, uuid, bytea, bigint, bigint, bigint, bigint, bigint, boolean, boolean, bytea)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE stamp timestamptz;
BEGIN
    PERFORM 1 FROM relay_hosted.active_owner_credentials a
        WHERE a.public_origin=$1 AND a.user_id=$2 AND a.credential_id=$3
            AND a.installation_epoch=$4 AND a.user_epoch=$5 AND a.revision=$6
        FOR UPDATE;
    stamp := clock_timestamp();
    IF NOT FOUND OR floor(extract(epoch FROM stamp)*1000)::bigint >= $7
        OR $8 NOT BETWEEN 0 AND 4294967295 THEN RETURN false; END IF;
    -- Only authenticator metadata may advance. The runtime cannot replace the public key.
    UPDATE relay_hosted.admin_credentials SET revision=revision+1, passkey=
        jsonb_set(jsonb_set(jsonb_set(passkey, '{cred,counter}', to_jsonb($8)),
            '{cred,backup_state}', to_jsonb($9)), '{cred,backup_eligible}', to_jsonb($10))
        WHERE credential_id=$3 AND revision=$6 AND (
            $8 > (passkey #>> '{cred,counter}')::bigint OR
            ($8=0 AND (passkey #>> '{cred,counter}')::bigint=0));
    IF NOT FOUND THEN RETURN false; END IF;
    DELETE FROM relay_hosted.admin_sessions WHERE expires_at <= stamp
        OR last_seen_at <= stamp - interval '20 minutes';
    DELETE FROM relay_hosted.admin_sessions WHERE verifier IN (
        SELECT verifier FROM relay_hosted.admin_sessions WHERE user_id=$2
        ORDER BY created_at DESC, verifier OFFSET 15
    );
    INSERT INTO relay_hosted.admin_sessions VALUES ($11,$2,$3,$4,$5,stamp,stamp,stamp+interval '8 hours');
    RETURN true;
END $$;

CREATE FUNCTION relay_hosted.read_admin_overview(text, bytea)
RETURNS text LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    UPDATE relay_hosted.admin_sessions sess SET last_seen_at=clock_timestamp()
    FROM relay_hosted.active_owner_credentials a WHERE sess.verifier=$2 AND a.public_origin=$1
        AND sess.user_id=a.user_id AND sess.credential_id=a.credential_id
        AND sess.installation_epoch=a.installation_epoch AND sess.user_epoch=a.user_epoch
        AND sess.expires_at > clock_timestamp() AND sess.last_seen_at > clock_timestamp()-interval '20 minutes'
    RETURNING a.mode
$$;

CREATE FUNCTION relay_hosted.end_admin_session(bytea)
RETURNS boolean LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog AS $$
    WITH removed AS (DELETE FROM relay_hosted.admin_sessions WHERE verifier=$1 RETURNING 1)
    SELECT EXISTS(SELECT FROM removed)
$$;

REVOKE ALL ON FUNCTION relay_hosted.owner_login_context(text),
    relay_hosted.owner_verifier(text,uuid,bytea,bigint,bigint),
    relay_hosted.issue_admin_session(text,uuid,bytea,bigint,bigint,bigint,bigint,bigint,boolean,boolean,bytea),
    relay_hosted.read_admin_overview(text,bytea), relay_hosted.end_admin_session(bytea) FROM PUBLIC;
