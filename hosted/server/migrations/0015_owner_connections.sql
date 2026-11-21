CREATE INDEX provider_slot_page ON relay_hosted.connection_secret_slots(installation_id,connection_id)
    WHERE purpose='provider';
CREATE INDEX text_source_label ON relay_hosted.text_inventory(source_id,checked_at DESC,id DESC);

-- Vault slots exist before qualification. A check, pool membership and a published offer are separate facts.
CREATE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (
        WITH page AS MATERIALIZED (
            SELECT s.installation_id,s.connection_id,s.generation IS NOT NULL AS stored
            FROM relay_hosted.installations h JOIN relay_hosted.connection_secret_slots s ON s.installation_id=h.id
            WHERE h.public_origin=origin AND s.purpose='provider' AND (after_id IS NULL OR s.connection_id>after_id)
            ORDER BY s.connection_id LIMIT 33
        ), visible AS (SELECT * FROM page ORDER BY connection_id LIMIT 32)
        SELECT jsonb_build_object(
            'items',coalesce(jsonb_agg(jsonb_build_object(
                'id',v.connection_id,'label',(SELECT i.source_label FROM relay_hosted.text_inventory i
                    WHERE i.source_id=v.connection_id ORDER BY i.checked_at DESC,i.id DESC LIMIT 1),
                'credential_state',CASE WHEN v.stored THEN 'stored' ELSE 'revoked' END,
                'proxy_bound',p.generation IS NOT NULL,
                'verified_models',ARRAY(SELECT DISTINCT i.model FROM relay_hosted.current_text_inventory i
                    WHERE i.installation_id=v.installation_id AND i.source_id=v.connection_id ORDER BY i.model)
            ) ORDER BY v.connection_id),'[]'::jsonb),
            'next_after',CASE WHEN (SELECT count(*) FROM page)=33
                THEN (SELECT connection_id FROM visible ORDER BY connection_id DESC LIMIT 1) END)
        FROM visible v LEFT JOIN relay_hosted.connection_secret_slots p
            ON (p.installation_id,p.connection_id,p.purpose)=(v.installation_id,v.connection_id,'proxy')
    );
END $$;
REVOKE ALL ON FUNCTION relay_hosted.owner_connections(text,bytea,uuid) FROM PUBLIC;
