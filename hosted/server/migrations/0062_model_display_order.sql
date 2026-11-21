CREATE TABLE relay_hosted.model_display_layout (
    installation_id uuid PRIMARY KEY REFERENCES relay_hosted.installations,
    revision bigint NOT NULL DEFAULT 0 CHECK (revision>=0),
    model_order text[] COLLATE "C" NOT NULL DEFAULT '{}' CHECK (cardinality(model_order)<=128)
);
INSERT INTO relay_hosted.model_display_layout(installation_id) SELECT id FROM relay_hosted.installations;

CREATE TABLE relay_hosted.reference_publishers (
    installation_id uuid NOT NULL REFERENCES relay_hosted.model_display_layout,
    model text COLLATE "C" NOT NULL CHECK (model ~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$'),
    publisher text COLLATE "C" NOT NULL CHECK (publisher ~ '^[a-z0-9][a-z0-9_-]{0,63}$' AND publisher<>'other'),
    PRIMARY KEY(installation_id,model)
);

CREATE INDEX text_inventory_display ON relay_hosted.text_inventory(scope_id,model,checked_at);

CREATE FUNCTION relay_hosted.model_display_rows(iid uuid,scope uuid,saved text[])
RETURNS TABLE(model text,publisher text,display_index bigint)
LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    WITH seen AS (
        SELECT i.model,min(i.checked_at) AS first_seen FROM relay_hosted.text_inventory i
        WHERE i.scope_id=scope GROUP BY i.model
    ), ranked AS (
        SELECT s.model,p.publisher,s.first_seen,a.ordinal,
            min(a.ordinal) OVER (PARTITION BY p.publisher) AS group_ordinal,
            min(s.first_seen) OVER (PARTITION BY p.publisher) AS group_seen
        FROM seen s LEFT JOIN relay_hosted.reference_publishers p
            ON p.installation_id=iid AND p.model=s.model
        LEFT JOIN unnest(saved) WITH ORDINALITY a(model,ordinal) ON s.model=a.model
    )
    SELECT r.model,r.publisher,row_number() OVER (ORDER BY r.group_ordinal NULLS LAST,
        r.group_seen,r.publisher NULLS LAST,r.ordinal NULLS LAST,r.first_seen,r.model)
    FROM ranked r
$$;

CREATE FUNCTION relay_hosted.model_display_view(layout relay_hosted.model_display_layout,scope uuid)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object(
        'revision',encode(sha256(convert_to(jsonb_build_array(layout.revision,scope,p.items)::text,'UTF8')),'hex'),
        'items',p.items)
    FROM (SELECT coalesce(jsonb_agg(jsonb_build_object('model',m.model,'publisher',m.publisher)
        ORDER BY m.display_index),'[]'::jsonb) AS items
        FROM relay_hosted.model_display_rows(layout.installation_id,scope,layout.model_order) m) p
$$;

CREATE FUNCTION relay_hosted.owner_model_display_order(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.owner_usage_session(origin,tag) IS NOT TRUE THEN RETURN NULL; END IF;
    RETURN (SELECT relay_hosted.model_display_view(l,s.qualification_scope_ref)
        FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.model_display_layout l ON l.installation_id=h.id WHERE h.public_origin=origin);
END $$;

CREATE FUNCTION relay_hosted.change_model_display_order(origin text,tag bytea,expected text,desired text[])
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; layout relay_hosted.model_display_layout; scope uuid;
    snapshot jsonb; ord text[]; stamp timestamptz;
BEGIN
    -- The shared service lock also fences inventory additions and scope changes.
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN NULL; END IF;
    IF cardinality(desired)>128 OR expected COLLATE "C" !~ '^[0-9a-f]{64}$' THEN RETURN '{"error":"invalid"}'; END IF;
    SELECT l.* INTO STRICT layout FROM relay_hosted.installations h
        JOIN relay_hosted.model_display_layout l ON l.installation_id=h.id WHERE h.public_origin=origin FOR UPDATE OF l;
    SELECT s.qualification_scope_ref INTO STRICT scope FROM relay_hosted.service_control s
        WHERE s.installation_id=layout.installation_id;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN NULL; END IF;
    snapshot:=relay_hosted.model_display_view(layout,scope);
    IF scope IS NULL OR snapshot->>'revision'<>expected THEN RETURN '{"error":"conflict"}'; END IF;
    IF (SELECT array_agg(a.model ORDER BY a.model COLLATE "C") FROM unnest(desired) a(model))
           IS DISTINCT FROM
       (SELECT array_agg(m.model ORDER BY m.model COLLATE "C")
        FROM jsonb_to_recordset(snapshot->'items') AS m(model text))
       OR EXISTS (
        SELECT FROM unnest(desired) WITH ORDINALITY AS a(model,ordinal)
        JOIN jsonb_to_recordset(snapshot->'items') AS m(model text,publisher text) ON m.model=a.model COLLATE "C"
        GROUP BY m.publisher COLLATE "C" HAVING count(*)<>max(a.ordinal)-min(a.ordinal)+1
    ) THEN RETURN '{"error":"invalid"}'; END IF;
    SELECT ARRAY(SELECT i->>'model' FROM jsonb_array_elements(snapshot->'items') i) INTO ord;
    IF desired=ord THEN RETURN snapshot; END IF;
    UPDATE relay_hosted.model_display_layout l SET model_order=desired,revision=l.revision+1
        WHERE l.installation_id=layout.installation_id RETURNING l.* INTO layout;
    RETURN relay_hosted.model_display_view(layout,scope);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.customer_text_models(origin text,provider uuid,binding bytea,token bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('model',l.model,'publisher',m.publisher,
        'protocol','responses','revision',l.revision::text,'effective_at_s',floor(extract(epoch FROM l.created_at))::bigint,
        'rates',relay_hosted.text_rates(o)) ORDER BY m.display_index),'[]')
        FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.model_display_layout d ON d.installation_id=h.id
        CROSS JOIN LATERAL relay_hosted.model_display_rows(h.id,s.qualification_scope_ref,d.model_order) m
        JOIN relay_hosted.customer_catalog_listings(origin,space) l ON l.model=m.model AND l.scope_id=s.qualification_scope_ref
        JOIN relay_hosted.text_offers o ON o.id=l.id
        WHERE h.public_origin=origin);
END $$;

REVOKE ALL ON FUNCTION relay_hosted.model_display_rows(uuid,uuid,text[]),
    relay_hosted.model_display_view(relay_hosted.model_display_layout,uuid),
    relay_hosted.owner_model_display_order(text,bytea),relay_hosted.change_model_display_order(text,bytea,text,text[]) FROM PUBLIC;
