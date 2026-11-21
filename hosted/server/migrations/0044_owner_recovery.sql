CREATE TABLE relay_hosted.owner_recoveries (
    operation_id uuid PRIMARY KEY CHECK (operation_id <> '00000000-0000-0000-0000-000000000000'),
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    owner_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    previous_owner_epoch bigint NOT NULL CHECK (previous_owner_epoch > 0),
    recovered_at timestamptz NOT NULL,
    UNIQUE (owner_id, previous_owner_epoch)
);
CREATE TRIGGER immutable_owner_recovery BEFORE UPDATE OR DELETE ON relay_hosted.owner_recoveries
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.owner_recovery_context(p_installation uuid, p_origin text, p_operation uuid)
RETURNS TABLE(installation_epoch bigint, owner_id uuid, owner_epoch bigint, service_revision bigint,
    deadline_ms bigint, committed boolean)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT i.authorization_epoch, u.id, u.authorization_epoch, s.revision,
        floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint + 180000,
        EXISTS (SELECT FROM relay_hosted.owner_recoveries r
            WHERE r.operation_id=p_operation AND r.installation_id=i.id AND r.owner_id=u.id)
    FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id=i.id
    JOIN relay_hosted.users u ON u.id=i.owner_user_id
    JOIN relay_hosted.admin_assignments a ON a.user_id=u.id
    WHERE i.singleton AND i.id=p_installation AND i.public_origin=p_origin AND i.setup_state='claimed'
        AND a.role='owner' AND a.revoked_at IS NULL AND u.disabled_at IS NULL
        AND s.mode IN ('setup','qualification','maintenance') AND s.restore_operation IS NULL
$$;

-- Only the offline recovery role calls this after proof of the replacement key.
CREATE FUNCTION relay_hosted.replace_owner_passkey(p_installation uuid, p_origin text, p_operation uuid,
    p_installation_epoch bigint, p_owner uuid, p_owner_epoch bigint, p_service_revision bigint,
    p_credential bytea, p_passkey jsonb, p_deadline_ms bigint)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz; stamp_ms bigint;
BEGIN
    PERFORM FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id=i.id
    JOIN relay_hosted.users u ON u.id=i.owner_user_id
    JOIN relay_hosted.admin_assignments a ON a.user_id=u.id
    WHERE i.singleton AND i.id=p_installation AND i.public_origin=p_origin AND i.setup_state='claimed'
        AND i.authorization_epoch=p_installation_epoch AND u.id=p_owner AND u.authorization_epoch=p_owner_epoch
        AND a.role='owner' AND a.revoked_at IS NULL AND u.disabled_at IS NULL
        AND s.revision=p_service_revision AND s.mode IN ('setup','qualification','maintenance')
        AND s.restore_operation IS NULL
    FOR UPDATE OF i,s,u,a;
    IF NOT FOUND THEN RETURN false; END IF;
    stamp := clock_timestamp();
    stamp_ms := floor(extract(epoch FROM stamp) * 1000)::bigint;
    IF p_deadline_ms <= stamp_ms OR p_deadline_ms > stamp_ms + 180000
        OR p_operation='00000000-0000-0000-0000-000000000000'
        OR EXISTS (SELECT FROM relay_hosted.owner_recoveries WHERE operation_id=p_operation)
    THEN RETURN false; END IF;

    INSERT INTO relay_hosted.admin_credentials (credential_id,user_id,passkey,created_at)
        VALUES (p_credential,p_owner,p_passkey,stamp) ON CONFLICT (credential_id) DO NOTHING;
    IF NOT FOUND THEN RETURN false; END IF;

    UPDATE relay_hosted.admin_credentials SET revoked_at=stamp,revision=revision+1
        WHERE user_id=p_owner AND revoked_at IS NULL AND credential_id<>p_credential;
    -- The owner epoch also fences old Desktop grants, without changing customer authority.
    UPDATE relay_hosted.users SET authorization_epoch=authorization_epoch+1 WHERE id=p_owner;
    DELETE FROM relay_hosted.admin_sessions WHERE user_id=p_owner;
    INSERT INTO relay_hosted.owner_recoveries VALUES (p_operation,p_installation,p_owner,p_owner_epoch,stamp);
    RETURN true;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_recovery_context(uuid,text,uuid),
    relay_hosted.replace_owner_passkey(uuid,text,uuid,bigint,uuid,bigint,bigint,bytea,jsonb,bigint) FROM PUBLIC;
