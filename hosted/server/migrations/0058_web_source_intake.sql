CREATE TABLE relay_hosted.web_source_intakes (
    installation_id uuid NOT NULL REFERENCES relay_hosted.installations,
    operation_id uuid NOT NULL,
    source_id uuid NOT NULL DEFAULT gen_random_uuid(),
    actor uuid NOT NULL REFERENCES relay_hosted.users,
    confirmed_by_passkey_id uuid NOT NULL REFERENCES relay_hosted.admin_credentials(passkey_id),
    label text NOT NULL,
    endpoint text NOT NULL,
    models text[] NOT NULL,
    proxied boolean NOT NULL,
    session_tag bytea,
    verifier_digest bytea,
    payload_mac bytea,
    expires_at timestamptz NOT NULL,
    committed_at timestamptz,
    PRIMARY KEY (installation_id, operation_id),
    UNIQUE (installation_id, source_id),
    CHECK (
        (committed_at IS NULL AND session_tag IS NOT NULL AND verifier_digest IS NOT NULL AND payload_mac IS NOT NULL)
        OR (committed_at IS NOT NULL AND committed_at <= expires_at
            AND session_tag IS NULL AND verifier_digest IS NULL AND payload_mac IS NULL)
    )
);

CREATE FUNCTION relay_hosted.approve_web_source(origin text,tag bytea,op uuid,name text,url text,declared text[],proxied boolean,
    verifier bytea,mac bytea,proof_credential bytea,proof_revision bigint,proof_counter bigint,
    proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE sess relay_hosted.admin_sessions; iid uuid; prior relay_hosted.web_source_intakes;
    stamp timestamptz; outcome text; signer uuid;
BEGIN
    IF origin NOT LIKE 'https://%' OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR name IS NULL OR name<>btrim(name) OR char_length(name) NOT BETWEEN 1 AND 80 OR name ~ '[[:cntrl:]]'
        OR url IS NULL OR octet_length(url)>256 OR url !~ '^https?://([a-z0-9.-]+|\[[a-f0-9:]+\])(:[0-9]+)?/v1$'
        OR declared IS NULL OR cardinality(declared)>128 OR coalesce(array_ndims(declared),1)<>1
        OR EXISTS(SELECT FROM unnest(declared) m WHERE m IS NULL OR m !~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$')
        OR (SELECT count(DISTINCT lower(m COLLATE "C")) FROM unnest(declared) m)<>cardinality(declared)
        OR proxied IS NULL OR octet_length(verifier) IS DISTINCT FROM 32 OR octet_length(mac) IS DISTINCT FROM 32
        THEN RETURN 'invalid'; END IF;
    sess:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF sess.verifier IS NULL THEN RETURN 'denied'; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    -- Serialize approvals and receipt transitions. Never take this lock before the owner fence.
    PERFORM pg_advisory_xact_lock(hashtextextended('web-source-intake:'||iid::text,0));
    stamp:=clock_timestamp();
    IF sess.expires_at<=stamp OR sess.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.web_source_intakes AS w WHERE (w.installation_id,w.operation_id)=(iid,op);
    IF FOUND THEN
        RETURN CASE WHEN (prior.actor,prior.label,prior.endpoint,prior.models,prior.proxied)
                IS DISTINCT FROM (sess.user_id,name,url,declared,proxied) THEN 'conflict'
            WHEN prior.committed_at IS NOT NULL THEN 'conflict'
            WHEN (prior.session_tag,prior.verifier_digest,prior.payload_mac) IS DISTINCT FROM (tag,verifier,mac)
                THEN 'conflict'
            WHEN prior.expires_at<=stamp THEN 'expired'
            ELSE 'approved' END;
    END IF;
    IF EXISTS(SELECT FROM relay_hosted.static_source_provisions p WHERE (p.installation_id,p.operation_id)=(iid,op))
        THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    DELETE FROM relay_hosted.web_source_intakes w
        WHERE w.installation_id=iid AND w.committed_at IS NULL AND w.expires_at<=stamp;
    IF (SELECT count(*) FROM relay_hosted.web_source_intakes w
        WHERE w.installation_id=iid AND w.committed_at IS NULL)>=24 THEN RETURN 'limit'; END IF;
    SELECT p.outcome,p.confirmer,p.verified_at INTO outcome,signer,stamp FROM relay_hosted.confirm_owner_payment(
        sess,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms) p;
    IF outcome<>'verified' THEN RETURN outcome; END IF;
    INSERT INTO relay_hosted.web_source_intakes(installation_id,operation_id,actor,confirmed_by_passkey_id,
        label,endpoint,models,proxied,session_tag,verifier_digest,payload_mac,expires_at)
        VALUES(iid,op,sess.user_id,signer,name,url,declared,proxied,tag,verifier,mac,stamp+interval '2 minutes');
    RETURN 'approved';
END $$;

CREATE FUNCTION relay_hosted.web_source_receipt(origin text,tag bytea,op uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE sess relay_hosted.admin_sessions; iid uuid; receipt relay_hosted.web_source_intakes;
BEGIN
    sess:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF sess.verifier IS NULL THEN RETURN NULL; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('web-source-intake:'||iid::text,0));
    IF sess.expires_at<=clock_timestamp() OR sess.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN NULL; END IF;
    SELECT * INTO receipt FROM relay_hosted.web_source_intakes w
        WHERE (w.installation_id,w.operation_id,w.actor)=(iid,op,sess.user_id);
    RETURN CASE WHEN receipt.committed_at IS NOT NULL THEN
            jsonb_build_object('state','saved','operation_id',op,'source_id',receipt.source_id)
        WHEN receipt.expires_at>clock_timestamp() THEN jsonb_build_object('state','pending','operation_id',op)
        ELSE jsonb_build_object('state','not_saved','operation_id',op) END;
END $$;

CREATE FUNCTION relay_hosted.web_source_access(origin text,iid uuid,op uuid,verifier bytea)
RETURNS TABLE(source_id uuid,proxied boolean,payload_mac bytea)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE permit relay_hosted.web_source_intakes; sess relay_hosted.admin_sessions; stamp timestamptz;
BEGIN
    SELECT * INTO permit FROM relay_hosted.web_source_intakes w WHERE (w.installation_id,w.operation_id)=(iid,op)
        AND w.verifier_digest=verifier AND w.committed_at IS NULL AND w.expires_at>clock_timestamp();
    IF NOT FOUND THEN RETURN; END IF;
    sess:=relay_hosted.lock_owner_payment_session(origin,permit.session_tag);
    IF sess.user_id IS DISTINCT FROM permit.actor THEN RETURN; END IF;
    -- The worker holds this fence through storage and acknowledgement. Expired receipts
    -- cannot race an uncommitted write, and an old permit cannot apply to a new row.
    PERFORM pg_advisory_xact_lock(hashtextextended('web-source-intake:'||iid::text,0));
    stamp:=clock_timestamp();
    IF sess.expires_at<=stamp OR sess.last_seen_at<=stamp-interval '20 minutes' THEN RETURN; END IF;
    PERFORM FROM relay_hosted.admin_credentials k WHERE k.passkey_id=permit.confirmed_by_passkey_id
        AND k.user_id=permit.actor AND k.revoked_at IS NULL FOR SHARE;
    IF NOT FOUND THEN RETURN; END IF;
    RETURN QUERY SELECT w.source_id,w.proxied,w.payload_mac FROM relay_hosted.web_source_intakes w
        WHERE (w.installation_id,w.operation_id,w.source_id,w.session_tag,w.verifier_digest)
            =(iid,op,permit.source_id,sess.verifier,verifier)
        AND w.committed_at IS NULL AND w.expires_at>clock_timestamp();
END $$;

CREATE FUNCTION relay_hosted.finish_web_source(origin text,iid uuid,op uuid,verifier bytea)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE source uuid; stamp timestamptz;
BEGIN
    SELECT a.source_id INTO source FROM relay_hosted.web_source_access(origin,iid,op,verifier) a;
    IF source IS NULL THEN RETURN false; END IF;
    stamp:=clock_timestamp();
    UPDATE relay_hosted.web_source_intakes w
        SET committed_at=stamp,session_tag=NULL,verifier_digest=NULL,payload_mac=NULL
        WHERE (w.installation_id,w.operation_id,w.source_id)=(iid,op,source) AND w.expires_at>stamp
        AND EXISTS(SELECT FROM relay_hosted.static_source_provisions p
            WHERE (p.installation_id,p.operation_id,p.source_id,p.proxied)=(iid,op,source,w.proxied));
    RETURN FOUND;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.source_copy_route(iid uuid,source uuid,endpoint text,proxied boolean)
RETURNS boolean LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT NOT EXISTS(SELECT FROM relay_hosted.desktop_source_copies c
        WHERE (c.installation_id,c.source_id)=($1,$2) AND c.committed_at IS NOT NULL
        AND (c.plan->>'endpoint',c.plan->'proxy'<>'null'::jsonb) IS DISTINCT FROM ($3,$4))
    AND NOT EXISTS(SELECT FROM relay_hosted.web_source_intakes w
        WHERE (w.installation_id,w.source_id)=($1,$2) AND w.committed_at IS NOT NULL
        AND (w.endpoint,w.proxied) IS DISTINCT FROM ($3,$4))
$$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid,inventory_view text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid;
BEGIN
    IF inventory_view IS NULL OR inventory_view NOT IN ('all','api','trash')
        OR relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    RETURN (SELECT jsonb_set(page,'{items}',(SELECT coalesce(jsonb_agg(item||jsonb_build_object(
        'serving',relay_hosted.source_serving_view(iid,(item->>'id')::uuid))
        ||CASE WHEN w.source_id IS NULL THEN '{}'::jsonb ELSE jsonb_build_object(
            'label',w.label,'endpoint',w.endpoint,'declared_models',w.models) END ORDER BY n),'[]'::jsonb)
        FROM jsonb_array_elements(page->'items') WITH ORDINALITY AS rows(item,n)
        LEFT JOIN relay_hosted.web_source_intakes w
            ON (w.installation_id,w.source_id)=(iid,(item->>'id')::uuid) AND w.committed_at IS NOT NULL))
        FROM relay_hosted.connection_page(iid,after_id,inventory_view) page);
END $$;

REVOKE ALL ON FUNCTION relay_hosted.approve_web_source(text,bytea,uuid,text,text,text[],boolean,bytea,bytea,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.web_source_receipt(text,bytea,uuid),
    relay_hosted.web_source_access(text,uuid,uuid,bytea),
    relay_hosted.finish_web_source(text,uuid,uuid,bytea) FROM PUBLIC;
