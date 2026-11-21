-- One atomic broadcast to the current qualification audience (at most 16).
CREATE TABLE relay_hosted.service_notices (
    id uuid PRIMARY KEY,
    revision integer NOT NULL CHECK (revision>0),
    title text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120 AND btrim(title)<>''),
    message text NOT NULL CHECK (char_length(message) BETWEEN 1 AND 4000 AND btrim(message)<>''),
    updated_at timestamptz NOT NULL,
    published_at timestamptz,
    expires_at timestamptz,
    scope_id uuid REFERENCES relay_hosted.configuration_revisions,
    recipient_count smallint,
    CHECK ((published_at IS NULL AND expires_at IS NULL AND scope_id IS NULL AND recipient_count IS NULL)
        OR (published_at IS NOT NULL AND published_at=updated_at
            AND expires_at IS NOT NULL AND expires_at>published_at AND scope_id IS NOT NULL
            AND recipient_count IS NOT NULL AND recipient_count BETWEEN 1 AND 16))
);
CREATE INDEX service_notice_expiry ON relay_hosted.service_notices(expires_at,id) WHERE published_at IS NOT NULL;

CREATE TABLE relay_hosted.service_notice_inbox (
    notice_id uuid NOT NULL REFERENCES relay_hosted.service_notices ON DELETE CASCADE,
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    read_at timestamptz,
    PRIMARY KEY(notice_id,workspace_id),
    FOREIGN KEY(workspace_id,user_id) REFERENCES relay_hosted.workspaces(id,owner_user_id)
);
CREATE INDEX service_notice_recipient ON relay_hosted.service_notice_inbox(workspace_id,notice_id);

-- Acknowledgement lookup also records the actor and action, not a second copy
-- of the title or body. It survives deletion of the draft or expired notice.
CREATE TABLE relay_hosted.service_notice_operations (
    operation_id uuid PRIMARY KEY,
    actor_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    notice_id uuid NOT NULL,
    action text NOT NULL CHECK (action IN ('save','discard','publish')),
    expected_revision integer NOT NULL CHECK (expected_revision>=0),
    content_digest bytea NOT NULL CHECK (octet_length(content_digest)=32),
    applied_at timestamptz NOT NULL
);
CREATE INDEX service_notice_operation_age ON relay_hosted.service_notice_operations(applied_at,operation_id);

CREATE FUNCTION relay_hosted.owner_service_notices(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN jsonb_build_object('items',coalesce((SELECT jsonb_agg(jsonb_build_object(
        'id',n.id,'revision',n.revision,'title',n.title,'message',n.message,
        'published_at_ms',floor(extract(epoch FROM n.published_at)*1000)::bigint,'recipient_count',n.recipient_count)
        ORDER BY n.published_at IS NOT NULL,n.updated_at DESC,n.id)
        FROM relay_hosted.service_notices n WHERE n.expires_at IS NULL OR n.expires_at>statement_timestamp()),'[]'));
END $$;

CREATE FUNCTION relay_hosted.change_service_notice(origin text,tag bytea,nid uuid,op uuid,expected integer,
    verb text,heading text,body text)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ctl relay_hosted.service_control; actor uuid; proof_at timestamptz; stamp timestamptz;
    prior relay_hosted.service_notice_operations; note relay_hosted.service_notices; digest bytea; recipients integer;
BEGIN
    -- Serialize scope changes, customer enrollment and owner notice mutations.
    -- There is no I/O under this lock. The bounded audience is fixed at commit.
    SELECT s.* INTO ctl FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
        WHERE i.singleton AND i.public_origin=origin AND i.setup_state='claimed' FOR UPDATE OF s;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    SELECT a.user_id,ss.created_at INTO actor,proof_at FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions ss ON ss.user_id=a.user_id AND ss.credential_id=a.credential_id
        WHERE ss.verifier=tag AND a.public_origin=origin AND ss.installation_epoch=a.installation_epoch
            AND ss.user_epoch=a.user_epoch AND ss.expires_at>clock_timestamp()
            AND ss.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF ss,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    IF nid IS NULL OR nid='00000000-0000-0000-0000-000000000000' OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 0 AND 2147483646 OR verb IS NULL
        OR (verb='save' AND (heading IS NULL OR body IS NULL OR char_length(heading) NOT BETWEEN 1 AND 120
            OR char_length(body) NOT BETWEEN 1 AND 4000 OR btrim(heading)='' OR btrim(body)=''))
        OR (verb<>'save' AND (verb NOT IN ('publish','discard') OR expected=0 OR heading IS NOT NULL OR body IS NOT NULL))
        THEN RETURN 'invalid'; END IF;

    stamp := clock_timestamp();
    digest := sha256(convert_to(jsonb_build_array(heading,body)::text,'UTF8'));
    SELECT * INTO prior FROM relay_hosted.service_notice_operations h WHERE h.operation_id=op;
    IF FOUND THEN
        IF prior.actor_id=actor AND prior.notice_id=nid AND prior.action=verb
            AND prior.expected_revision=expected AND prior.content_digest=digest THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF (SELECT count(*) FROM relay_hosted.service_notice_operations)>=2048 THEN RETURN 'capacity'; END IF;
    SELECT * INTO note FROM relay_hosted.service_notices n WHERE n.id=nid;
    IF expected=0 AND NOT FOUND THEN
        IF (SELECT count(*) FROM relay_hosted.service_notices WHERE published_at IS NULL)>=16 THEN RETURN 'capacity'; END IF;
        INSERT INTO relay_hosted.service_notices(id,revision,title,message,updated_at)
            VALUES(nid,1,heading,body,stamp);
    ELSIF NOT FOUND OR note.published_at IS NOT NULL OR note.revision<>expected THEN
        RETURN 'conflict';
    ELSIF verb='save' THEN
        UPDATE relay_hosted.service_notices SET title=heading,message=body,revision=expected+1,updated_at=stamp WHERE id=nid;
    ELSIF verb='discard' THEN
        DELETE FROM relay_hosted.service_notices WHERE id=nid;
    ELSE
        IF proof_at<=stamp-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
        IF (SELECT count(*) FROM relay_hosted.service_notices WHERE expires_at>stamp)>=32 THEN RETURN 'capacity'; END IF;
        IF ctl.mode<>'qualification' THEN RETURN 'no_audience'; END IF;
        INSERT INTO relay_hosted.service_notice_inbox(notice_id,workspace_id,user_id)
            SELECT nid,w.id,w.owner_user_id FROM relay_hosted.qualification_subjects q
            JOIN relay_hosted.configuration_revisions r ON r.id=q.scope_id
            JOIN relay_hosted.workspaces w ON w.id=q.workspace_id
            JOIN relay_hosted.users u ON u.id=w.owner_user_id
            JOIN relay_hosted.external_identities e ON e.provider_id=r.provider_id
              AND e.subject_digest=q.subject_digest AND e.user_id=u.id
            WHERE q.scope_id=ctl.qualification_scope_ref AND r.expires_at>stamp
                AND u.disabled_at IS NULL AND e.revoked_at IS NULL
            ORDER BY w.id FOR SHARE OF u,e;
        GET DIAGNOSTICS recipients = ROW_COUNT;
        IF recipients=0 THEN RETURN 'no_audience'; END IF;
        UPDATE relay_hosted.service_notices SET revision=expected+1,updated_at=stamp,published_at=stamp,
            expires_at=stamp+interval '720 hours',scope_id=ctl.qualification_scope_ref,recipient_count=recipients WHERE id=nid;
    END IF;
    INSERT INTO relay_hosted.service_notice_operations VALUES(op,actor,nid,verb,expected,digest,stamp);
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.customer_service_notices(origin text,provider uuid,binding bytea,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,tag) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN jsonb_build_object('items',coalesce((SELECT jsonb_agg(jsonb_build_object(
        'id',n.id,'title',n.title,'message',n.message,'read',inbox.read_at IS NOT NULL,
        'published_at_ms',floor(extract(epoch FROM n.published_at)*1000)::bigint) ORDER BY n.published_at DESC,n.id)
        FROM relay_hosted.customer_sessions sess JOIN relay_hosted.service_notice_inbox inbox
            ON inbox.workspace_id=space AND inbox.user_id=sess.user_id
        JOIN relay_hosted.service_notices n ON n.id=inbox.notice_id AND n.scope_id=sess.scope_id
        WHERE sess.verifier=tag AND n.expires_at>clock_timestamp()),'[]'));
END $$;

CREATE FUNCTION relay_hosted.read_service_notices(origin text,provider uuid,binding bytea,tag bytea,ids uuid[])
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; matched integer; stamp timestamptz;
BEGIN
    IF cardinality(ids) NOT BETWEEN 1 AND 32 OR array_position(ids,NULL) IS NOT NULL THEN RETURN 'invalid'; END IF;
    PERFORM FROM relay_hosted.service_control s JOIN relay_hosted.installations i ON i.id=s.installation_id
        WHERE i.singleton AND i.public_origin=origin FOR SHARE OF s;
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,tag) a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    stamp := clock_timestamp();
    SELECT count(*) INTO matched FROM (
        SELECT inbox.notice_id FROM relay_hosted.customer_sessions sess JOIN relay_hosted.service_notice_inbox inbox
            ON inbox.workspace_id=space AND inbox.user_id=sess.user_id
        JOIN relay_hosted.service_notices n ON n.id=inbox.notice_id AND n.scope_id=sess.scope_id
        WHERE sess.verifier=tag AND n.expires_at>stamp AND n.id=ANY(ids)
        ORDER BY inbox.notice_id FOR UPDATE OF inbox
    ) own;
    IF matched<>cardinality(ids) THEN RETURN 'missing'; END IF;
    UPDATE relay_hosted.service_notice_inbox SET read_at=stamp
        WHERE workspace_id=space AND notice_id=ANY(ids) AND read_at IS NULL;
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.prune_service_notices()
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    DELETE FROM relay_hosted.service_notices WHERE id IN (
        SELECT id FROM relay_hosted.service_notices WHERE published_at IS NOT NULL AND expires_at<=statement_timestamp()
        ORDER BY expires_at,id LIMIT 32 FOR UPDATE SKIP LOCKED);
    DELETE FROM relay_hosted.service_notice_operations WHERE operation_id IN (
        SELECT operation_id FROM relay_hosted.service_notice_operations WHERE applied_at<=statement_timestamp()-interval '720 hours'
        ORDER BY applied_at,operation_id LIMIT 128 FOR UPDATE SKIP LOCKED);
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_service_notices(text,bytea),
    relay_hosted.change_service_notice(text,bytea,uuid,uuid,integer,text,text,text),
    relay_hosted.customer_service_notices(text,uuid,bytea,bytea),
    relay_hosted.read_service_notices(text,uuid,bytea,bytea,uuid[]),
    relay_hosted.prune_service_notices() FROM PUBLIC;
