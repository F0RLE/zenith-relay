CREATE TABLE relay_hosted.customer_keys (
    id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES relay_hosted.workspaces,
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    issue_operation uuid NOT NULL,
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 80 AND label=btrim(label) AND label !~ '[[:cntrl:]]'),
    replaces uuid,
    requested_expiry bigint,
    verifier bytea NOT NULL UNIQUE CHECK (octet_length(verifier)=32),
    user_epoch bigint NOT NULL,
    installation_epoch bigint NOT NULL,
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL CHECK (expires_at>created_at),
    revoked_at timestamptz,
    UNIQUE (workspace_id,issue_operation),
    UNIQUE (workspace_id,id),
    FOREIGN KEY (workspace_id,replaces) REFERENCES relay_hosted.customer_keys(workspace_id,id)
);
CREATE INDEX customer_key_page ON relay_hosted.customer_keys(workspace_id,created_at DESC,id DESC);
CREATE INDEX customer_key_scope ON relay_hosted.customer_keys(workspace_id,scope_id);

-- Serialize a change with pause, suspension, logout, and other key changes in
-- the workspace. Check session and scope expiry *after* those locks are acquired.
CREATE FUNCTION relay_hosted.key_authority(origin text, provider uuid, binding bytea, token bytea, writing boolean)
RETURNS relay_hosted.customer_sessions LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE session_row relay_hosted.customer_sessions;
BEGIN
    PERFORM FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON i.id=s.installation_id
        WHERE i.public_origin=origin AND s.mode='qualification' FOR SHARE OF i,s;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT ss.* INTO session_row FROM relay_hosted.customer_sessions ss
        JOIN relay_hosted.users u ON u.id=ss.user_id
        JOIN relay_hosted.external_identities e ON e.user_id=u.id AND e.provider_id=ss.provider_id
        WHERE ss.verifier=token AND e.subject_digest=ss.subject_digest FOR SHARE OF u,e;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF writing THEN
        PERFORM FROM relay_hosted.workspaces w WHERE w.id=session_row.workspace_id FOR NO KEY UPDATE;
    END IF;
    PERFORM FROM relay_hosted.customer_account(origin,provider,binding,token);
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN session_row;
END $$;

CREATE FUNCTION relay_hosted.key_summary(k relay_hosted.customer_keys, scope uuid)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',k.id,'operation_id',k.issue_operation,'name',k.label,
        'prefix','rlk_'||left(replace(k.id::text,'-',''),12),'replaces_key_id',k.replaces,
        'expires_at_s',floor(extract(epoch FROM k.expires_at))::bigint,
        'status',CASE WHEN k.revoked_at IS NOT NULL THEN 'revoked'
            WHEN k.scope_id<>scope OR k.expires_at<=statement_timestamp() THEN 'expired' ELSE 'active' END)
$$;

CREATE FUNCTION relay_hosted.read_customer_keys(origin text, provider uuid, binding bytea, token bytea, op uuid, cursor_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; boundary timestamptz;
BEGIN
    ss := relay_hosted.key_authority(origin,provider,binding,token,false);
    IF ss.verifier IS NULL THEN RETURN NULL; END IF;
    IF cursor_id IS NOT NULL THEN
        SELECT k.created_at INTO boundary FROM relay_hosted.customer_keys k WHERE k.workspace_id=ss.workspace_id AND k.id=cursor_id;
        IF NOT FOUND THEN RETURN '[]'::jsonb; END IF;
    END IF;
    RETURN (SELECT coalesce(jsonb_agg(page.summary ORDER BY page.created_at DESC,page.id DESC),'[]') FROM
        (SELECT k.id,k.created_at,relay_hosted.key_summary(k,ss.scope_id) AS summary FROM relay_hosted.customer_keys k
            WHERE k.workspace_id=ss.workspace_id AND (op IS NULL OR k.issue_operation=op)
              AND (cursor_id IS NULL OR (k.created_at,k.id)<(boundary,cursor_id))
            ORDER BY k.created_at DESC,k.id DESC LIMIT 33) page);
END $$;

CREATE FUNCTION relay_hosted.issue_customer_key(origin text, provider uuid, binding bytea, token bytea,
    op uuid, name text, original uuid, requested bigint, new_id uuid, new_verifier bytea)
RETURNS TABLE(outcome text, summary jsonb) LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; k relay_hosted.customer_keys; until_at timestamptz; stamp timestamptz;
BEGIN
    ss := relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO k FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.issue_operation=op;
    IF FOUND THEN
        IF k.label=name AND k.replaces IS NOT DISTINCT FROM original AND k.requested_expiry IS NOT DISTINCT FROM requested THEN
            outcome:='existing'; summary:=relay_hosted.key_summary(k,ss.scope_id);
        ELSE outcome:='conflict'; END IF;
        RETURN NEXT; RETURN;
    END IF;
    SELECT r.expires_at INTO STRICT until_at FROM relay_hosted.configuration_revisions r WHERE r.id=ss.scope_id;
    IF original IS NOT NULL THEN
        SELECT * INTO k FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.id=original;
        IF NOT FOUND THEN outcome:='missing'; RETURN NEXT; RETURN; END IF;
        IF k.revoked_at IS NOT NULL OR k.scope_id<>ss.scope_id OR k.expires_at<=clock_timestamp() THEN
            outcome:='conflict'; RETURN NEXT; RETURN;
        END IF;
        until_at:=least(until_at,k.expires_at);
    END IF;
    stamp:=clock_timestamp();
    IF requested IS NOT NULL THEN
        IF requested<=extract(epoch FROM stamp) OR requested>extract(epoch FROM until_at) THEN
            outcome:='expiry'; RETURN NEXT; RETURN;
        END IF;
        until_at:=to_timestamp(requested);
    END IF;
    IF until_at<=stamp THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    IF (SELECT count(*) FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.scope_id=ss.scope_id)>=32 THEN
        outcome:='limit'; RETURN NEXT; RETURN;
    END IF;
    INSERT INTO relay_hosted.customer_keys(id,workspace_id,scope_id,issue_operation,label,replaces,requested_expiry,verifier,
        user_epoch,installation_epoch,created_at,expires_at)
        VALUES(new_id,ss.workspace_id,ss.scope_id,op,name,original,requested,new_verifier,
            ss.user_epoch,ss.installation_epoch,stamp,until_at) RETURNING * INTO k;
    outcome:='issued'; summary:=relay_hosted.key_summary(k,ss.scope_id); RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.revoke_customer_key(origin text, provider uuid, binding bytea, token bytea, target uuid)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    UPDATE relay_hosted.customer_keys k SET revoked_at=coalesce(k.revoked_at,clock_timestamp())
        WHERE k.workspace_id=ss.workspace_id AND k.id=target;
    RETURN CASE WHEN FOUND THEN 'revoked' ELSE 'missing' END;
END $$;

-- Private credential check, not a dispatch permit or an HTTP endpoint.
CREATE FUNCTION relay_hosted.qualified_key_workspace(origin text, key_id uuid, proof bytea)
RETURNS uuid LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT k.workspace_id FROM relay_hosted.customer_keys k
        JOIN relay_hosted.configuration_revisions r ON r.id=k.scope_id
        JOIN relay_hosted.service_control s ON s.qualification_scope_ref=r.id
        JOIN relay_hosted.installations i ON i.id=s.installation_id
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=r.id AND q.workspace_id=k.workspace_id
        JOIN relay_hosted.external_identities e ON e.provider_id=r.provider_id AND e.subject_digest=q.subject_digest
        JOIN relay_hosted.workspaces w ON w.id=k.workspace_id AND w.owner_user_id=e.user_id
        JOIN relay_hosted.users u ON u.id=e.user_id
    WHERE i.public_origin=origin AND s.mode='qualification' AND k.id=key_id AND k.verifier=proof
        AND k.installation_epoch=i.authorization_epoch AND k.user_epoch=u.authorization_epoch
        AND k.revoked_at IS NULL AND e.revoked_at IS NULL AND u.disabled_at IS NULL
        AND k.expires_at>statement_timestamp() AND r.expires_at>statement_timestamp()
$$;
REVOKE ALL ON FUNCTION relay_hosted.key_authority(text,uuid,bytea,bytea,boolean),
    relay_hosted.key_summary(relay_hosted.customer_keys,uuid),
    relay_hosted.read_customer_keys(text,uuid,bytea,bytea,uuid,uuid),
    relay_hosted.issue_customer_key(text,uuid,bytea,bytea,uuid,text,uuid,bigint,uuid,bytea),
    relay_hosted.revoke_customer_key(text,uuid,bytea,bytea,uuid),
    relay_hosted.qualified_key_workspace(text,uuid,bytea) FROM PUBLIC;
