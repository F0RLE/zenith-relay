CREATE FUNCTION relay_hosted.owner_pool_activity(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE scope uuid;
BEGIN
    -- Polling must not renew the owner session or reserve provider capacity.
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    SELECT s.qualification_scope_ref INTO STRICT scope
        FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
        WHERE h.public_origin=origin;
    RETURN jsonb_build_object(
        'strategy',CASE WHEN scope IS NOT NULL THEN 'automatic' END,
        'items',coalesce((
            SELECT jsonb_agg(jsonb_build_object('source_id',a.source_id,'source',a.label,'model',a.model,'requests',a.requests)
                ORDER BY a.source_id,a.model)
            FROM (
                SELECT i.source_id,min(i.source_label) AS label,o.model,count(*) AS requests
                FROM relay_hosted.text_offers o JOIN relay_hosted.text_requests r ON r.offer_id=o.id
                JOIN relay_hosted.text_purchase_attempts p ON p.attempt_id=r.attempt_id
                JOIN relay_hosted.text_inventory i ON (i.id,i.scope_id)=(p.item,o.scope_id)
                WHERE o.scope_id=scope AND r.execution='dispatched' AND r.closed_at IS NULL
                    AND r.claimed_at IS NOT NULL AND r.close_by>statement_timestamp()
                GROUP BY i.source_id,o.model
            ) a
        ),'[]'::jsonb)
    );
END $$;

REVOKE ALL ON FUNCTION relay_hosted.owner_pool_activity(text,bytea) FROM PUBLIC;
