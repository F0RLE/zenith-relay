-- A receipt records a completed creation, not current credential validity or pool admission.
CREATE TABLE relay_hosted.static_source_provisions (
    installation_id uuid NOT NULL REFERENCES relay_hosted.vault_bindings,
    operation_id uuid NOT NULL,
    source_id uuid NOT NULL,
    proxied boolean NOT NULL,
    payload_tag bytea NOT NULL CHECK (octet_length(payload_tag)=32),
    committed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (installation_id, operation_id),
    UNIQUE (installation_id, source_id)
);
CREATE TRIGGER immutable_static_source_provision BEFORE UPDATE OR DELETE ON relay_hosted.static_source_provisions
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

-- A missing/revoked proxy invalidates the qualification; it never implies a direct route.
CREATE OR REPLACE FUNCTION relay_hosted.text_source_matches(iid uuid,source uuid,provider_rev bigint,proxy_rev bigint,proxied boolean)
RETURNS boolean LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.connection_secret_slots s WHERE s.installation_id=$1
        AND s.connection_id=$2 AND s.purpose='provider' AND s.revision=$3 AND s.generation IS NOT NULL)
    AND coalesce((SELECT p.revision=$4 AND (p.generation IS NOT NULL)=$5 FROM relay_hosted.connection_secret_slots p
        WHERE p.installation_id=$1 AND p.connection_id=$2 AND p.purpose='proxy'),$4=0 AND NOT $5)
    AND NOT EXISTS(SELECT FROM relay_hosted.static_source_provisions r WHERE r.installation_id=$1
        AND r.source_id=$2 AND r.proxied<>$5)
$$;
