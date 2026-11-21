-- Installed sandbox profiles are immutable, authenticated by the private payment
-- worker's role. The browser never provides merchant identities or credentials.
CREATE TABLE relay_hosted.checkout_adapter_profiles (
    binding_id uuid PRIMARY KEY,
    environment text NOT NULL DEFAULT 'sandbox' CHECK (environment='sandbox'),
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 64 AND label=btrim(label) AND label !~ '[[:cntrl:]]'),
    methods jsonb NOT NULL CHECK (jsonb_typeof(methods)='array' AND jsonb_array_length(methods) BETWEEN 1 AND 16),
    FOREIGN KEY (binding_id,environment) REFERENCES relay_hosted.merchant_bindings(id,environment)
);
CREATE TRIGGER fixed_checkout_profile BEFORE UPDATE OR DELETE ON relay_hosted.checkout_adapter_profiles
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.register_checkout_profile(origin text,binding uuid,heading text,capabilities jsonb)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE normalized jsonb;
BEGIN
    IF NOT relay_hosted.local_checkout(origin)
        OR NOT EXISTS (SELECT FROM relay_hosted.closed_installation i WHERE i.public_origin=origin)
        OR NOT EXISTS (SELECT FROM relay_hosted.merchant_bindings b WHERE b.id=binding AND b.environment='sandbox')
        OR jsonb_typeof(capabilities)<>'array' OR jsonb_array_length(capabilities) NOT BETWEEN 1 AND 16
        THEN RETURN false; END IF;
    IF (SELECT count(DISTINCT m.code) FROM jsonb_to_recordset(capabilities) m(code text))
            <>jsonb_array_length(capabilities)
        OR EXISTS (SELECT FROM jsonb_to_recordset(capabilities) m(code text,label text,min_credit_minor integer,
            max_credit_minor integer,fee_minor integer,custom_amount boolean,merchant_pays boolean,customer_pays boolean)
            WHERE NOT ((m.code COLLATE "C" ~ '^[a-z0-9][a-z0-9_-]{0,63}$'
                AND char_length(m.label) BETWEEN 1 AND 64 AND m.label=btrim(m.label) AND m.label !~ '[[:cntrl:]]'
                AND m.min_credit_minor BETWEEN 1 AND m.max_credit_minor AND m.max_credit_minor<=1000000
                AND m.fee_minor BETWEEN 0 AND 1000000 AND m.custom_amount IS NOT NULL
                AND m.merchant_pays IS NOT NULL AND m.customer_pays IS NOT NULL
                AND (m.merchant_pays OR m.customer_pays)) IS TRUE))
        THEN RETURN false; END IF;
    SELECT jsonb_agg(to_jsonb(m)-'n' ORDER BY m.n) INTO normalized
        FROM ROWS FROM (jsonb_to_recordset(capabilities) AS (code text,label text,min_credit_minor integer,
            max_credit_minor integer,fee_minor integer,custom_amount boolean,merchant_pays boolean,customer_pays boolean))
        WITH ORDINALITY m(code,label,min_credit_minor,max_credit_minor,fee_minor,custom_amount,merchant_pays,customer_pays,n);
    IF normalized<>capabilities OR char_length(heading) NOT BETWEEN 1 AND 64
        OR heading<>btrim(heading) OR heading ~ '[[:cntrl:]]' THEN RETURN false; END IF;
    INSERT INTO relay_hosted.checkout_adapter_profiles(binding_id,label,methods)
        VALUES(binding,heading,normalized) ON CONFLICT (binding_id) DO NOTHING;
    RETURN EXISTS (SELECT FROM relay_hosted.checkout_adapter_profiles p
        WHERE p.binding_id=binding AND p.label=heading AND p.methods=capabilities);
END $$;

ALTER TABLE relay_hosted.checkout_methods
    ADD COLUMN fee_payer text NOT NULL DEFAULT 'customer' CHECK (fee_payer IN ('merchant','customer')),
    ADD COLUMN merchant_fee_minor integer NOT NULL DEFAULT 0 CHECK (merchant_fee_minor BETWEEN 0 AND 1000000),
    ADD CHECK ((fee_payer='merchant' AND fee_minor=0) OR (fee_payer='customer' AND merchant_fee_minor=0));
CREATE TRIGGER fixed_checkout_fee BEFORE UPDATE OF fee_payer,merchant_fee_minor ON relay_hosted.checkout_methods
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE TABLE relay_hosted.checkout_builder (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision>0),
    draft jsonb NOT NULL DEFAULT '{"connections":[],"methods":[],"minimum_credit_minor":100,
        "quick_amounts":[1000,2000,5000],"custom_amount":true,"open_limit":3}',
    checked_until timestamptz
);
INSERT INTO relay_hosted.checkout_builder DEFAULT VALUES;

-- Audit and replay are committed with the effect. A publication captures the
-- exact settings and method order, separately from the mutable draft.
CREATE TABLE relay_hosted.checkout_builder_operations (
    operation_id uuid PRIMARY KEY,
    actor_id uuid NOT NULL REFERENCES relay_hosted.admin_assignments,
    expected_revision bigint NOT NULL,
    action text NOT NULL CHECK (action IN ('save','check','publish','disable')),
    digest bytea NOT NULL CHECK (octet_length(digest)=32),
    target_method uuid REFERENCES relay_hosted.checkout_methods,
    publication jsonb,
    applied_at timestamptz NOT NULL,
    CHECK ((action='disable')=(target_method IS NOT NULL)),
    CHECK ((action='publish')=(publication IS NOT NULL))
);
CREATE TRIGGER fixed_checkout_builder_operation BEFORE UPDATE OR DELETE ON relay_hosted.checkout_builder_operations
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.checkout_draft_methods(draft jsonb)
RETURNS TABLE (binding_id uuid, method_code text, label text, display_order smallint,
    min_credit_minor integer,max_credit_minor integer,fee_minor integer,fee_payer text,merchant_fee_minor integer,
    quick_amounts integer[],custom_amount boolean,eligible boolean)
LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT p.binding_id,m.code,m.label,(s.position-1)::smallint,
        limits.minimum,m.max_credit_minor,CASE WHEN s.item->>'fee_payer'='customer' THEN m.fee_minor ELSE 0 END,
        s.item->>'fee_payer',CASE WHEN s.item->>'fee_payer'='merchant' THEN m.fee_minor ELSE 0 END,
        amounts.quick,limits.custom,limits.minimum<=m.max_credit_minor AND (limits.custom OR cardinality(amounts.quick)>0)
    FROM jsonb_array_elements(draft->'methods') WITH ORDINALITY s(item,position)
    JOIN relay_hosted.checkout_adapter_profiles p ON p.binding_id=(s.item->>'binding_id')::uuid
        AND (draft->'connections') ? (s.item->>'binding_id')
    CROSS JOIN LATERAL jsonb_to_recordset(p.methods) m(code text,label text,min_credit_minor integer,
        max_credit_minor integer,fee_minor integer,custom_amount boolean,merchant_pays boolean,customer_pays boolean)
    CROSS JOIN LATERAL (SELECT greatest((draft->>'minimum_credit_minor')::integer,m.min_credit_minor) minimum,
        (draft->>'custom_amount')::boolean AND m.custom_amount custom) limits
    CROSS JOIN LATERAL (SELECT ARRAY(SELECT a.value::integer
        FROM jsonb_array_elements_text(draft->'quick_amounts') WITH ORDINALITY a(value,n)
        WHERE a.value::integer BETWEEN limits.minimum AND m.max_credit_minor ORDER BY a.n) quick) amounts
    WHERE m.code=s.item->>'code' AND (CASE s.item->>'fee_payer'
        WHEN 'merchant' THEN m.merchant_pays WHEN 'customer' THEN m.customer_pays ELSE false END)
$$;

CREATE FUNCTION relay_hosted.valid_checkout_draft(draft jsonb) RETURNS boolean
LANGUAGE plpgsql STABLE STRICT SET search_path=pg_catalog AS $$
DECLARE minimum integer:=(draft->>'minimum_credit_minor')::integer; custom boolean:=(draft->>'custom_amount')::boolean;
BEGIN
    IF NOT (jsonb_typeof(draft->'connections')='array' AND jsonb_array_length(draft->'connections')<=8
        AND jsonb_typeof(draft->'methods')='array' AND jsonb_array_length(draft->'methods')<=16
        AND minimum BETWEEN 1 AND 1000000 AND (draft->>'open_limit')::integer BETWEEN 1 AND 10
        AND jsonb_typeof(draft->'custom_amount')='boolean'
        AND jsonb_typeof(draft->'quick_amounts')='array' AND jsonb_array_length(draft->'quick_amounts')<=8
        AND (custom OR jsonb_array_length(draft->'quick_amounts')>0)) IS TRUE
        THEN RETURN false; END IF;
    RETURN (SELECT count(*)=jsonb_array_length(draft->'connections')
        FROM relay_hosted.checkout_adapter_profiles p WHERE (draft->'connections') ? p.binding_id::text)
        AND (SELECT count(DISTINCT (m.binding_id,m.method_code))=jsonb_array_length(draft->'methods')
            FROM relay_hosted.checkout_draft_methods(draft) m)
        AND (SELECT count(DISTINCT a::integer)=jsonb_array_length(draft->'quick_amounts')
            FROM jsonb_array_elements_text(draft->'quick_amounts') a WHERE a::integer BETWEEN minimum AND 1000000);
END $$;

CREATE FUNCTION relay_hosted.owner_checkout_builder(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT jsonb_build_object('revision',b.revision::text,'draft',b.draft,
        'editable',relay_hosted.local_checkout(origin),
        'checked_until_s',floor(extract(epoch FROM b.checked_until))::bigint,
        'eligible_count',CASE WHEN b.checked_until>statement_timestamp() THEN availability.eligible END,
        'methods_without_amounts',availability.omitted,
        'published_open_limit',p.open_limit,
        'adapters',CASE WHEN relay_hosted.local_checkout(origin) THEN coalesce((SELECT jsonb_agg(
            jsonb_build_object('id',a.binding_id,'label',a.label,'methods',a.methods) ORDER BY a.label,a.binding_id)
            FROM relay_hosted.checkout_adapter_profiles a),'[]') ELSE '[]' END,
        'published',coalesce((SELECT jsonb_agg(jsonb_build_object(
            'id',m.id,'label',m.label,'fee_payer',m.fee_payer,'processing_fee_minor',m.fee_minor+m.merchant_fee_minor)
            ORDER BY m.position,m.id) FROM relay_hosted.checkout_methods m WHERE m.disabled_at IS NULL
            AND relay_hosted.local_checkout(origin)),'[]'))
        FROM relay_hosted.checkout_builder b CROSS JOIN relay_hosted.checkout_policy p
        CROSS JOIN LATERAL (
            SELECT count(*) FILTER (WHERE m.eligible) eligible,
                coalesce(jsonb_agg(jsonb_build_object('binding_id',m.binding_id,'code',m.method_code)
                    ORDER BY m.display_order) FILTER (WHERE NOT m.eligible),'[]') omitted
            FROM relay_hosted.checkout_draft_methods(b.draft) m
        ) availability);
END $$;

CREATE FUNCTION relay_hosted.publish_checkout(draft jsonb,stamp timestamptz) RETURNS void
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
    -- Preserve method IDs across reordering or a cap change. A changed term
    -- gets a new ID so unconfirmed quotes cannot inherit it.
    WITH wanted AS MATERIALIZED (SELECT * FROM relay_hosted.checkout_draft_methods(draft) WHERE eligible)
    UPDATE relay_hosted.checkout_methods m SET disabled_at=stamp WHERE m.disabled_at IS NULL
        AND NOT EXISTS (SELECT FROM wanted w
            WHERE (m.merchant_binding_id,m.method_code,m.label,m.min_credit_minor,m.max_credit_minor,
                m.fee_minor,m.fee_payer,m.merchant_fee_minor,m.quick_amounts,m.custom_amount)
            =(w.binding_id,w.method_code COLLATE "C",w.label,w.min_credit_minor,w.max_credit_minor,
                w.fee_minor,w.fee_payer,w.merchant_fee_minor,w.quick_amounts,w.custom_amount));
    INSERT INTO relay_hosted.checkout_methods(merchant_binding_id,method_code,label,position,min_credit_minor,
        max_credit_minor,fee_minor,fee_payer,merchant_fee_minor,quick_amounts,custom_amount)
        SELECT binding_id,method_code,label,display_order,min_credit_minor,max_credit_minor,fee_minor,fee_payer,
            merchant_fee_minor,quick_amounts,custom_amount
        FROM relay_hosted.checkout_draft_methods(draft) WHERE eligible
        ON CONFLICT (merchant_binding_id,method_code) WHERE disabled_at IS NULL
        DO UPDATE SET position=excluded.position;
    UPDATE relay_hosted.checkout_policy SET open_limit=(draft->>'open_limit')::smallint;
END $$;

CREATE FUNCTION relay_hosted.change_checkout_builder(origin text,tag bytea,op uuid,expected bigint,
    verb text,draft_input jsonb,target uuid)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE b relay_hosted.checkout_builder; prior relay_hosted.checkout_builder_operations;
    actor uuid; proof_at timestamptz; stamp timestamptz; fingerprint bytea; retired relay_hosted.checkout_methods;
BEGIN
    SELECT a.user_id,sess.created_at INTO actor,proof_at FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions sess ON sess.user_id=a.user_id AND sess.credential_id=a.credential_id
        WHERE sess.verifier=tag AND a.public_origin=origin AND sess.installation_epoch=a.installation_epoch
            AND sess.user_epoch=a.user_epoch AND sess.expires_at>clock_timestamp()
            AND sess.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF sess,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    IF op IS NULL OR op='00000000-0000-0000-0000-000000000000' OR expected IS NULL OR expected<1
        OR verb IS NULL OR verb NOT IN ('save','check','publish','disable')
        OR (verb='save')<>(draft_input IS NOT NULL) OR (verb='disable')<>(target IS NOT NULL)
        THEN RETURN 'invalid'; END IF;
    IF NOT relay_hosted.local_checkout(origin) THEN RETURN 'unavailable'; END IF;
    SELECT * INTO STRICT b FROM relay_hosted.checkout_builder FOR UPDATE;
    stamp:=clock_timestamp();
    fingerprint:=sha256(convert_to(jsonb_build_array(draft_input,target)::text,'UTF8'));
    SELECT * INTO prior FROM relay_hosted.checkout_builder_operations WHERE operation_id=op;
    IF FOUND THEN
        IF (actor,expected,verb,fingerprint)=(prior.actor_id,prior.expected_revision,prior.action,prior.digest)
            THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF b.revision<>expected THEN RETURN 'conflict'; END IF;
    IF verb IN ('publish','disable') AND proof_at<=stamp-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;

    IF verb='save' THEN
        IF NOT relay_hosted.valid_checkout_draft(draft_input) THEN RETURN 'invalid'; END IF;
        UPDATE relay_hosted.checkout_builder SET draft=draft_input,revision=revision+1,checked_until=NULL;
    ELSIF verb='check' THEN
        IF NOT relay_hosted.valid_checkout_draft(b.draft) THEN RETURN 'invalid'; END IF;
        UPDATE relay_hosted.checkout_builder SET checked_until=stamp+interval '10 minutes';
    ELSIF verb='publish' THEN
        IF b.checked_until IS NULL OR b.checked_until<=stamp THEN RETURN 'check_required'; END IF;
        PERFORM relay_hosted.publish_checkout(b.draft,stamp);
        UPDATE relay_hosted.checkout_builder SET revision=revision+1,checked_until=NULL;
    ELSE
        UPDATE relay_hosted.checkout_methods SET disabled_at=stamp WHERE id=target AND disabled_at IS NULL
            RETURNING * INTO retired;
        IF NOT FOUND THEN RETURN 'conflict'; END IF;
        UPDATE relay_hosted.checkout_builder SET revision=revision+1,checked_until=NULL,
            draft=jsonb_set(b.draft,'{methods}',coalesce((SELECT jsonb_agg(s.item ORDER BY s.n)
                FROM jsonb_array_elements(b.draft->'methods') WITH ORDINALITY s(item,n)
                WHERE (s.item->>'binding_id',(s.item->>'code') COLLATE "C")
                    <>(retired.merchant_binding_id::text,retired.method_code)),'[]'));
    END IF;
    INSERT INTO relay_hosted.checkout_builder_operations VALUES (op,actor,expected,verb,fingerprint,target,
        CASE WHEN verb='publish' THEN b.draft END,stamp);
    RETURN 'applied';
END $$;

REVOKE ALL ON FUNCTION relay_hosted.register_checkout_profile(text,uuid,text,jsonb),
    relay_hosted.checkout_draft_methods(jsonb),relay_hosted.valid_checkout_draft(jsonb),
    relay_hosted.publish_checkout(jsonb,timestamptz),relay_hosted.owner_checkout_builder(text,bytea),
    relay_hosted.change_checkout_builder(text,bytea,uuid,bigint,text,jsonb,uuid) FROM PUBLIC;
