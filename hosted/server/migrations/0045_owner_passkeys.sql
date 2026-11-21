ALTER TABLE relay_hosted.admin_credentials
    ADD COLUMN passkey_id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    ADD COLUMN label text CHECK (length(label) BETWEEN 1 AND 48 AND btrim(label)=label AND label !~ '[[:cntrl:]]');

CREATE TABLE relay_hosted.owner_passkey_changes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    confirmed_by uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    action text NOT NULL CHECK (action IN ('added','revoked')),
    at timestamptz NOT NULL,
    CHECK (passkey_id <> confirmed_by)
);
CREATE TRIGGER immutable_owner_passkey_change BEFORE UPDATE OR DELETE ON relay_hosted.owner_passkey_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.owner_passkey_session(p_origin text,p_tag bytea)
RETURNS relay_hosted.admin_sessions LANGUAGE sql STRICT SET search_path=pg_catalog AS $$
    SELECT sess.* FROM relay_hosted.active_owner_credentials a
    JOIN relay_hosted.admin_sessions sess ON (sess.user_id,sess.credential_id,sess.installation_epoch,sess.user_epoch)
        =(a.user_id,a.credential_id,a.installation_epoch,a.user_epoch)
    WHERE a.public_origin=p_origin AND sess.verifier=p_tag AND sess.expires_at>clock_timestamp()
        AND sess.last_seen_at>clock_timestamp()-interval '20 minutes'
$$;

CREATE FUNCTION relay_hosted.owner_passkeys(p_origin text,p_tag bytea)
RETURNS jsonb LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('passkeys',(
        SELECT jsonb_agg(jsonb_build_object('id',k.passkey_id,'label',k.label,
            'created_ms',floor(extract(epoch FROM k.created_at)*1000)::bigint,
            'current',k.credential_id=sess.credential_id) ORDER BY k.created_at,k.passkey_id)
        FROM relay_hosted.admin_credentials k WHERE k.user_id=sess.user_id AND k.revoked_at IS NULL
    ))
    FROM relay_hosted.owner_passkey_session(p_origin,p_tag) sess WHERE sess.verifier IS NOT NULL
$$;

-- Verifiers stay in the server. This query is not the browser's list projection.
CREATE FUNCTION relay_hosted.owner_passkey_context(p_origin text,p_tag bytea)
RETURNS TABLE(owner_id uuid,deadline_ms bigint,passkey_id uuid,revision bigint,passkey jsonb)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT sess.user_id,floor(extract(epoch FROM statement_timestamp())*1000)::bigint+180000,
        k.passkey_id,k.revision,k.passkey
    FROM relay_hosted.owner_passkey_session(p_origin,p_tag) sess
    JOIN relay_hosted.admin_credentials k ON k.user_id=sess.user_id AND k.revoked_at IS NULL
$$;

CREATE FUNCTION relay_hosted.change_owner_passkey(p_origin text,p_tag bytea,p_owner uuid,
    p_confirmed_by bytea,p_revision bigint,p_counter bigint,p_backup boolean,p_eligible boolean,p_deadline_ms bigint,
    p_revoke uuid,p_new_credential bytea,p_new_passkey jsonb,p_label text)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE sess relay_hosted.admin_sessions; stamp timestamptz; stamp_ms bigint;
    target uuid; target_credential bytea; confirmer uuid; active_count bigint;
BEGIN
    -- Use the same installation lock as local recovery and recheck the session
    -- and both credentials after acquiring it.
    PERFORM FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
        WHERE i.singleton AND i.owner_user_id=p_owner AND i.public_origin=p_origin AND s.restore_operation IS NULL
        FOR UPDATE OF i,s;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT t.* INTO sess FROM relay_hosted.admin_sessions t
        WHERE t.verifier=p_tag AND t.user_id=p_owner
            AND (relay_hosted.owner_passkey_session(p_origin,p_tag)).user_id=p_owner
        FOR UPDATE;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    stamp := clock_timestamp();
    stamp_ms := floor(extract(epoch FROM stamp)*1000)::bigint;
    IF p_deadline_ms IS NULL OR p_deadline_ms<=stamp_ms OR p_deadline_ms>stamp_ms+180000
        THEN RETURN 'expired'; END IF;

    -- Lock the active set so a concurrent change also invalidates stale
    -- repeatable-read snapshots before applying the count guard.
    SELECT count(*) INTO active_count FROM (
        SELECT credential_id FROM relay_hosted.admin_credentials
        WHERE user_id=p_owner AND revoked_at IS NULL ORDER BY credential_id FOR UPDATE
    ) keys;
    IF p_revoke IS NOT NULL THEN
        IF p_new_credential IS NOT NULL OR p_new_passkey IS NOT NULL OR p_label IS NOT NULL THEN RETURN 'changed'; END IF;
        SELECT credential_id INTO target_credential FROM relay_hosted.admin_credentials
            WHERE user_id=p_owner AND passkey_id=p_revoke AND revoked_at IS NULL;
        IF NOT FOUND THEN RETURN 'changed'; END IF;
        IF active_count<=1 THEN RETURN 'last_passkey'; END IF;
        IF target_credential=p_confirmed_by THEN RETURN 'changed'; END IF;
    ELSE
        IF active_count>=8 THEN RETURN 'passkey_limit'; END IF;
        IF p_new_credential IS NULL OR p_new_passkey IS NULL
            OR EXISTS (SELECT FROM relay_hosted.admin_credentials WHERE credential_id=p_new_credential)
        THEN RETURN 'changed'; END IF;
    END IF;

    -- The verifier checked the assertion. Advance only metadata and require its
    -- exact still-active revision, never replace the confirming public key.
    UPDATE relay_hosted.admin_credentials SET revision=revision+1, passkey=
        jsonb_set(jsonb_set(jsonb_set(passkey,'{cred,counter}',to_jsonb(p_counter)),
            '{cred,backup_state}',to_jsonb(p_backup)),'{cred,backup_eligible}',to_jsonb(p_eligible))
        WHERE user_id=p_owner AND credential_id=p_confirmed_by AND revoked_at IS NULL AND revision=p_revision
            AND p_backup IS NOT NULL AND p_eligible IS NOT NULL AND p_counter BETWEEN 0 AND 4294967295
            AND (p_counter>(passkey #>> '{cred,counter}')::bigint
                OR (p_counter=0 AND (passkey #>> '{cred,counter}')::bigint=0))
        RETURNING passkey_id INTO confirmer;
    IF NOT FOUND THEN RETURN 'changed'; END IF;

    IF p_revoke IS NOT NULL THEN
        UPDATE relay_hosted.admin_credentials SET revoked_at=stamp,revision=revision+1 WHERE passkey_id=p_revoke;
        DELETE FROM relay_hosted.admin_sessions WHERE user_id=p_owner AND credential_id=target_credential;
        INSERT INTO relay_hosted.owner_passkey_changes(owner_id,passkey_id,confirmed_by,action,at)
            VALUES (p_owner,p_revoke,confirmer,'revoked',stamp);
        RETURN CASE WHEN sess.credential_id=target_credential THEN 'signed_out' ELSE 'applied' END;
    END IF;
    INSERT INTO relay_hosted.admin_credentials (credential_id,user_id,passkey,label,created_at)
        VALUES (p_new_credential,p_owner,p_new_passkey,p_label,stamp) RETURNING passkey_id INTO target;
    INSERT INTO relay_hosted.owner_passkey_changes(owner_id,passkey_id,confirmed_by,action,at)
        VALUES (p_owner,target,confirmer,'added',stamp);
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_passkey_session(text,bytea),
    relay_hosted.owner_passkeys(text,bytea),relay_hosted.owner_passkey_context(text,bytea),
    relay_hosted.change_owner_passkey(text,bytea,uuid,bytea,bigint,bigint,boolean,boolean,bigint,uuid,bytea,jsonb,text) FROM PUBLIC;
