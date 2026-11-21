CREATE TABLE relay_hosted.users (
    id uuid PRIMARY KEY,
    authorization_epoch bigint NOT NULL DEFAULT 1 CHECK (authorization_epoch > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE relay_hosted.admin_credentials (
    credential_id bytea PRIMARY KEY CHECK (octet_length(credential_id) BETWEEN 1 AND 1023),
    user_id uuid NOT NULL REFERENCES relay_hosted.users(id),
    passkey jsonb NOT NULL CHECK (jsonb_typeof(passkey) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE relay_hosted.admin_assignments (
    user_id uuid PRIMARY KEY REFERENCES relay_hosted.users(id),
    role text NOT NULL UNIQUE CHECK (role = 'owner'),
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE relay_hosted.installations
    DROP CONSTRAINT installations_setup_state_check,
    ADD COLUMN owner_user_id uuid REFERENCES relay_hosted.admin_assignments(user_id),
    ADD CONSTRAINT owner_binding CHECK (
        (setup_state = 'unclaimed' AND owner_user_id IS NULL) OR
        (setup_state = 'claimed' AND owner_user_id IS NOT NULL)
    );

CREATE OR REPLACE VIEW relay_hosted.closed_installation AS
    SELECT i.public_origin FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id = i.id
    WHERE i.singleton AND s.mode = 'setup';

CREATE VIEW relay_hosted.pending_owner AS
    SELECT i.id, i.authorization_epoch, i.public_origin FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id = i.id
    WHERE i.singleton AND i.setup_state = 'unclaimed' AND s.mode = 'setup';

-- Only the local setup role receives EXECUTE. The HTTP runtime has no writes.
CREATE FUNCTION relay_hosted.bind_first_owner(uuid, bigint, text, uuid, bytea, jsonb, bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    PERFORM 1 FROM relay_hosted.installations i JOIN relay_hosted.service_control s
        ON s.installation_id = i.id WHERE i.id = $1 AND i.authorization_epoch = $2
        AND i.public_origin = $3 AND i.setup_state = 'unclaimed' AND s.mode = 'setup'
        FOR UPDATE OF i, s;
    IF NOT FOUND OR floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint >= $7
        THEN RETURN false; END IF;
    INSERT INTO relay_hosted.users (id) VALUES ($4);
    INSERT INTO relay_hosted.admin_credentials (credential_id, user_id, passkey) VALUES ($5, $4, $6);
    INSERT INTO relay_hosted.admin_assignments (user_id, role) VALUES ($4, 'owner');
    UPDATE relay_hosted.installations SET owner_user_id = $4, setup_state = 'claimed',
        authorization_epoch = authorization_epoch + 1 WHERE id = $1;
    RETURN true;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint) FROM PUBLIC;
