CREATE FUNCTION relay_hosted.connection_page(iid uuid,after_id uuid,inventory_view text) RETURNS jsonb
LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    WITH page AS MATERIALIZED (
        SELECT s.connection_id,s.generation IS NOT NULL AS stored FROM relay_hosted.connection_secret_slots s
        WHERE s.installation_id=iid AND s.purpose='provider' AND (after_id IS NULL OR s.connection_id>after_id)
          AND (inventory_view='all' OR (inventory_view='trash')=
            (s.generation IS NOT NULL AND EXISTS (
                SELECT FROM relay_hosted.source_serving v
                WHERE (v.installation_id,v.source_id)=(iid,s.connection_id) AND v.action='trash'
            )))
        ORDER BY s.connection_id LIMIT 33
    ), visible AS (SELECT * FROM page ORDER BY connection_id LIMIT 32)
    SELECT jsonb_build_object('items',coalesce(jsonb_agg(jsonb_build_object(
        'id',v.connection_id,'label',coalesce(c.plan->>'label',(SELECT i.source_label FROM relay_hosted.text_inventory i
            WHERE i.source_id=v.connection_id ORDER BY i.checked_at DESC,i.id DESC LIMIT 1)),
        'credential_state',CASE WHEN v.stored THEN 'stored' ELSE 'revoked' END,'proxy_bound',p.generation IS NOT NULL,
        'verified_models',ARRAY(SELECT DISTINCT i.model FROM relay_hosted.current_text_inventory i
            WHERE i.installation_id=iid AND i.source_id=v.connection_id ORDER BY i.model)
    ) ORDER BY v.connection_id),'[]'::jsonb),'next_after',CASE WHEN (SELECT count(*) FROM page)=33
        THEN (SELECT connection_id FROM visible ORDER BY connection_id DESC LIMIT 1) END)
    FROM visible v LEFT JOIN relay_hosted.connection_secret_slots p
        ON (p.installation_id,p.connection_id,p.purpose)=(iid,v.connection_id,'proxy')
    LEFT JOIN relay_hosted.desktop_source_copies c
        ON (c.installation_id,c.source_id)=(iid,v.connection_id) AND c.committed_at IS NOT NULL
$$;

CREATE OR REPLACE FUNCTION relay_hosted.connection_page(iid uuid,after_id uuid) RETURNS jsonb
LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT relay_hosted.connection_page(iid,after_id,'all')
$$;

CREATE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid,inventory_view text)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid;
BEGIN
    IF inventory_view IS NULL OR inventory_view NOT IN ('all','api','trash')
        OR relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    SELECT id INTO STRICT iid FROM relay_hosted.installations WHERE public_origin=origin;
    RETURN (SELECT jsonb_set(page,'{items}',(SELECT coalesce(jsonb_agg(item||jsonb_build_object(
        'serving',relay_hosted.source_serving_view(iid,(item->>'id')::uuid)) ORDER BY n),'[]'::jsonb)
        FROM jsonb_array_elements(page->'items') WITH ORDINALITY AS rows(item,n)))
        FROM relay_hosted.connection_page(iid,after_id,inventory_view) page);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_connections(origin text,tag bytea,after_id uuid)
RETURNS jsonb LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT relay_hosted.owner_connections(origin,tag,after_id,'all')
$$;

REVOKE ALL ON FUNCTION relay_hosted.connection_page(uuid,uuid,text),
    relay_hosted.owner_connections(text,bytea,uuid,text) FROM PUBLIC;
