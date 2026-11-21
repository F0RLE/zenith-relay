-- Private storage worker only. The web and setup roles get no grants here.
CREATE TABLE relay_hosted.vault_bindings (
    installation_id uuid PRIMARY KEY REFERENCES relay_hosted.installations,
    key_version smallint NOT NULL CHECK (key_version=1),
    marker_digest bytea NOT NULL CHECK (octet_length(marker_digest)=32)
);

CREATE TABLE relay_hosted.connection_secret_slots (
    installation_id uuid NOT NULL REFERENCES relay_hosted.vault_bindings,
    connection_id uuid NOT NULL,
    purpose text NOT NULL CHECK (purpose IN ('provider','proxy')),
    revision bigint NOT NULL CHECK (revision > 0),
    generation uuid UNIQUE,
    manifest_digest bytea,
    PRIMARY KEY (installation_id, connection_id, purpose),
    CHECK ((generation IS NULL) = (manifest_digest IS NULL)),
    CHECK (octet_length(manifest_digest)=32)
);
