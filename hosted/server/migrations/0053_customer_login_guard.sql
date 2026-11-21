DROP FUNCTION relay_hosted.customer_login_context(text);

-- No subject is known at this point. Read the service gate before contacting
-- the issuer; finish_customer_login still locks and checks the verified subject.
CREATE FUNCTION relay_hosted.customer_login_context(origin text, provider uuid, config_tag bytea,
    expected_revision bigint, attempt_deadline_ms bigint)
RETURNS TABLE(revision bigint, deadline_ms bigint)
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT s.revision,d.deadline
    FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
    LEFT JOIN relay_hosted.configuration_revisions r ON r.id=s.qualification_scope_ref
    LEFT JOIN relay_hosted.identity_providers p ON p.id=r.provider_id
    CROSS JOIN (SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint AS now_ms) t
    CROSS JOIN LATERAL (SELECT least(t.now_ms+180000,
        floor(extract(epoch FROM r.expires_at)*1000)::bigint,$5) AS deadline) d
    WHERE i.singleton AND i.public_origin=$1 AND i.setup_state='claimed' AND s.restore_operation IS NULL
        AND EXISTS (SELECT FROM relay_hosted.active_owner_credentials a WHERE a.public_origin=$1)
        AND (($4 IS NULL AND $5 IS NULL) OR (s.revision=$4 AND $5 IS NOT NULL))
        AND d.deadline > t.now_ms AND octet_length($3)=32 AND $2 IS NOT NULL
        AND (s.mode IN ('setup','maintenance') OR (
            s.mode='qualification' AND r.mode='qualification' AND r.revision=s.revision
            AND p.id=$2 AND p.binding=$3))
$$;
REVOKE ALL ON FUNCTION relay_hosted.customer_login_context(text,uuid,bytea,bigint,bigint) FROM PUBLIC;
