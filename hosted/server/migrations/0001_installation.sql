CREATE TABLE relay_hosted.installations (
    id uuid PRIMARY KEY DEFAULT pg_catalog.gen_random_uuid(),
    singleton boolean NOT NULL UNIQUE DEFAULT true CHECK (singleton),
    public_origin text NOT NULL,
    setup_state text NOT NULL DEFAULT 'unclaimed' CHECK (setup_state = 'unclaimed'),
    authorization_epoch bigint NOT NULL DEFAULT 1 CHECK (authorization_epoch > 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE relay_hosted.service_control (
    installation_id uuid PRIMARY KEY REFERENCES relay_hosted.installations(id),
    mode text NOT NULL DEFAULT 'setup' CHECK (mode = 'setup'),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    admission_epoch bigint NOT NULL DEFAULT 1 CHECK (admission_epoch > 0),
    changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE VIEW relay_hosted.closed_installation AS
    SELECT i.public_origin
    FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id = i.id
    WHERE i.singleton AND i.setup_state = 'unclaimed' AND s.mode = 'setup';

-- The migration runner supplies the validated origin on this connection.
-- Schema, identity and the admission fence are committed together, once.
WITH created AS (
    INSERT INTO relay_hosted.installations (public_origin)
    VALUES (current_setting('relay_hosted.initial_public_origin'))
    RETURNING id
)
INSERT INTO relay_hosted.service_control (installation_id) SELECT id FROM created;
