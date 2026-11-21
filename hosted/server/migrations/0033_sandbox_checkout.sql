-- Until merchant qualification, only the local sandbox can publish checkout methods.
ALTER TABLE relay_hosted.merchant_bindings ADD UNIQUE (id,environment);

CREATE TABLE relay_hosted.checkout_policy (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    open_limit smallint NOT NULL DEFAULT 3 CHECK (open_limit BETWEEN 1 AND 10)
);
INSERT INTO relay_hosted.checkout_policy DEFAULT VALUES;

CREATE TABLE relay_hosted.checkout_methods (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    merchant_binding_id uuid NOT NULL,
    environment text NOT NULL DEFAULT 'sandbox' CHECK (environment='sandbox'),
    method_code relay_hosted.payment_reference NOT NULL,
    label text NOT NULL CHECK (char_length(label) BETWEEN 1 AND 64 AND label=btrim(label) AND label !~ '[[:cntrl:]]'),
    position smallint NOT NULL CHECK (position>=0),
    min_credit_minor integer NOT NULL CHECK (min_credit_minor>0),
    max_credit_minor integer NOT NULL CHECK (max_credit_minor BETWEEN min_credit_minor AND 1000000),
    fee_minor integer NOT NULL CHECK (fee_minor BETWEEN 0 AND 1000000),
    quick_amounts integer[] NOT NULL DEFAULT '{}',
    custom_amount boolean NOT NULL DEFAULT true,
    disabled_at timestamptz,
    FOREIGN KEY (merchant_binding_id,environment) REFERENCES relay_hosted.merchant_bindings(id,environment),
    UNIQUE (id,merchant_binding_id),
    CHECK (cardinality(quick_amounts)<=8 AND array_position(quick_amounts,NULL) IS NULL
        AND min_credit_minor<=ALL(quick_amounts) AND max_credit_minor>=ALL(quick_amounts)
        AND (custom_amount OR cardinality(quick_amounts)>0))
);
CREATE UNIQUE INDEX enabled_checkout_method ON relay_hosted.checkout_methods(merchant_binding_id,method_code)
    WHERE disabled_at IS NULL;
CREATE TRIGGER fixed_checkout_method BEFORE UPDATE OF id,merchant_binding_id,environment,method_code,label,
    min_credit_minor,max_credit_minor,fee_minor,quick_amounts,custom_amount ON relay_hosted.checkout_methods
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER retired_checkout_method BEFORE UPDATE OF disabled_at ON relay_hosted.checkout_methods
    FOR EACH ROW WHEN (OLD.disabled_at IS NOT NULL AND NEW.disabled_at IS DISTINCT FROM OLD.disabled_at)
    EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE TABLE relay_hosted.checkout_quotes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES relay_hosted.wallets,
    method_id uuid NOT NULL REFERENCES relay_hosted.checkout_methods,
    credit_minor integer NOT NULL CHECK (credit_minor>0),
    expires_at timestamptz NOT NULL,
    UNIQUE (workspace_id,method_id,credit_minor)
);
CREATE INDEX checkout_quote_expiry ON relay_hosted.checkout_quotes(expires_at,id);

ALTER TABLE relay_hosted.payment_intents
    ADD COLUMN checkout_method_id uuid,
    ADD COLUMN checkout_claimed_at timestamptz,
    ADD COLUMN invoice_expires_at timestamptz,
    ADD COLUMN invoice_closed_at timestamptz,
    ADD FOREIGN KEY (checkout_method_id,merchant_binding_id)
        REFERENCES relay_hosted.checkout_methods(id,merchant_binding_id),
    ADD CHECK (checkout_method_id IS NOT NULL OR
        (checkout_claimed_at IS NULL AND invoice_expires_at IS NULL AND invoice_closed_at IS NULL)),
    ADD CHECK (checkout_method_id IS NULL OR (quoted_currency='USD'
        AND ((merchant_reference IS NULL AND invoice_expires_at IS NULL) OR
            (merchant_reference IS NOT NULL AND invoice_expires_at IS NOT NULL
                AND invoice_expires_at>created_at AND checkout_claimed_at IS NOT NULL))
        AND (invoice_closed_at IS NULL OR checkout_claimed_at IS NOT NULL)));
CREATE TRIGGER fixed_checkout_terms BEFORE UPDATE OF checkout_method_id ON relay_hosted.payment_intents
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE INDEX checkout_queue ON relay_hosted.payment_intents(merchant_binding_id,created_at,id)
    WHERE checkout_method_id IS NOT NULL AND checkout_claimed_at IS NULL;
CREATE INDEX checkout_history ON relay_hosted.payment_intents(workspace_id,created_at DESC,id DESC)
    WHERE checkout_method_id IS NOT NULL;

CREATE FUNCTION relay_hosted.local_checkout(origin text) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT origin ~ '^http://(localhost|127[.]0[.]0[.]1|\[::1\])(:[0-9]+)?$'
$$;

CREATE FUNCTION relay_hosted.customer_checkout_methods(origin text,provider uuid,binding bytea,token bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF (relay_hosted.key_authority(origin,provider,binding,token,false)).verifier IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',m.id,'label',m.label,
        'min_credit_micro_usd',(m.min_credit_minor::bigint*10000)::text,
        'max_credit_micro_usd',(m.max_credit_minor::bigint*10000)::text,'custom_amount',m.custom_amount,
        'quick_credit_micro_usd',to_jsonb(ARRAY(SELECT (a::bigint*10000)::text
            FROM unnest(m.quick_amounts) WITH ORDINALITY AS choice(a,n) GROUP BY a ORDER BY min(n)))
        ) ORDER BY m.position,m.id),'[]')
        FROM (SELECT * FROM relay_hosted.checkout_methods WHERE disabled_at IS NULL AND relay_hosted.local_checkout(origin)
            ORDER BY position,id LIMIT 16) m);
END $$;

CREATE FUNCTION relay_hosted.quote_checkout(origin text,provider uuid,binding bytea,token bytea,method uuid,credit bigint)
RETURNS TABLE(outcome text,payload jsonb) LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; m relay_hosted.checkout_methods; q relay_hosted.checkout_quotes;
    stamp timestamptz;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO m FROM relay_hosted.checkout_methods x WHERE x.id=method AND x.disabled_at IS NULL
        AND relay_hosted.local_checkout(origin) FOR SHARE;
    IF NOT FOUND THEN outcome:='unavailable'; RETURN NEXT; RETURN; END IF;
    IF credit%10000<>0 OR credit/10000 NOT BETWEEN m.min_credit_minor AND m.max_credit_minor
        OR NOT (m.custom_amount OR credit/10000=ANY(m.quick_amounts))
        THEN outcome:='amount'; RETURN NEXT; RETURN; END IF;
    stamp:=clock_timestamp();
    DELETE FROM relay_hosted.checkout_quotes x WHERE x.workspace_id=ss.workspace_id AND x.expires_at<=stamp;
    SELECT * INTO q FROM relay_hosted.checkout_quotes x
        WHERE x.workspace_id=ss.workspace_id AND x.method_id=m.id AND x.credit_minor=credit/10000;
    IF NOT FOUND THEN
        IF (SELECT count(*) FROM relay_hosted.checkout_quotes x WHERE x.workspace_id=ss.workspace_id)>=16
            THEN outcome:='quote_limit'; RETURN NEXT; RETURN; END IF;
        INSERT INTO relay_hosted.checkout_quotes(workspace_id,method_id,credit_minor,expires_at)
            VALUES(ss.workspace_id,m.id,credit/10000,stamp+interval '5 minutes') RETURNING * INTO q;
    END IF;
    outcome:='ready';
    payload:=jsonb_build_object('id',q.id,'method_id',m.id,'currency','USD','credit_micro_usd',credit::text,
        'payable_minor',(q.credit_minor+m.fee_minor)::text,'fee_minor',m.fee_minor::text,
        'expires_at_s',floor(extract(epoch FROM q.expires_at))::bigint);
    RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.checkout_summary(i relay_hosted.payment_intents,label text,receipt uuid)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',i.id,'operation_id',i.operation_id,'quote_id',i.quote_id,
        'label',label,'credit_micro_usd',i.credit_micro_usd::text,'currency',i.quoted_currency,
        'payable_minor',i.quoted_amount_minor::text,'receipt_id',receipt,
        'created_at_s',floor(extract(epoch FROM i.created_at))::bigint,
        'state',CASE WHEN receipt IS NOT NULL THEN 'paid' WHEN i.invoice_closed_at IS NOT NULL THEN 'closed'
            WHEN i.invoice_expires_at>statement_timestamp() THEN 'awaiting_payment' ELSE 'processing' END)
$$;

CREATE FUNCTION relay_hosted.confirm_checkout(origin text,provider uuid,binding bytea,token bytea,quote uuid,op uuid)
RETURNS TABLE(outcome text,payload jsonb) LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; m relay_hosted.checkout_methods;
    q relay_hosted.checkout_quotes; i relay_hosted.payment_intents; slots integer; capacity smallint;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO i FROM relay_hosted.payment_intents x WHERE x.workspace_id=ss.workspace_id AND x.operation_id=op;
    IF FOUND THEN
        IF i.quote_id<>quote OR i.checkout_method_id IS NULL THEN outcome:='conflict';
        ELSE
            outcome:='accepted';
            SELECT relay_hosted.checkout_summary(i,mth.label,j.id) INTO payload FROM relay_hosted.checkout_methods mth
                LEFT JOIN relay_hosted.journal_transactions j ON j.payment_id=i.id WHERE mth.id=i.checkout_method_id;
        END IF;
        RETURN NEXT; RETURN;
    END IF;
    IF EXISTS (SELECT FROM relay_hosted.payment_intents x WHERE x.workspace_id=ss.workspace_id AND x.quote_id=quote)
        THEN outcome:='conflict'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO q FROM relay_hosted.checkout_quotes x WHERE x.workspace_id=ss.workspace_id AND x.id=quote;
    IF NOT FOUND THEN outcome:='missing'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO m FROM relay_hosted.checkout_methods x WHERE x.id=q.method_id AND x.disabled_at IS NULL
        AND relay_hosted.local_checkout(origin) FOR SHARE;
    IF NOT FOUND THEN outcome:='unavailable'; RETURN NEXT; RETURN; END IF;
    SELECT open_limit INTO STRICT capacity FROM relay_hosted.checkout_policy FOR SHARE;
    -- Credit posting takes the same wallet lock. Do not call the merchant under it.
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=ss.workspace_id FOR NO KEY UPDATE;
    IF ss.expires_at<=clock_timestamp() OR NOT EXISTS (SELECT FROM relay_hosted.configuration_revisions r
        WHERE r.id=ss.scope_id AND r.expires_at>clock_timestamp())
        THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    IF q.expires_at<=clock_timestamp() THEN outcome:='expired'; RETURN NEXT; RETURN; END IF;
    SELECT count(*)::integer INTO slots FROM relay_hosted.payment_intents x
        WHERE x.workspace_id=ss.workspace_id AND x.invoice_closed_at IS NULL
        AND NOT EXISTS (SELECT FROM relay_hosted.journal_transactions j WHERE j.payment_id=x.id);
    IF slots>=capacity THEN
        outcome:='invoice_limit'; payload:=jsonb_build_object('open_count',slots,'open_limit',capacity);
        RETURN NEXT; RETURN;
    END IF;
    INSERT INTO relay_hosted.payment_intents(workspace_id,operation_id,quote_id,merchant_binding_id,
        checkout_method_id,quoted_currency,quoted_amount_minor,credit_micro_usd,created_at)
        VALUES(ss.workspace_id,op,q.id,m.merchant_binding_id,m.id,'USD',q.credit_minor+m.fee_minor,
            q.credit_minor::bigint*10000,clock_timestamp()) RETURNING * INTO i;
    DELETE FROM relay_hosted.checkout_quotes x WHERE x.id=q.id;
    outcome:='accepted'; payload:=relay_hosted.checkout_summary(i,m.label,NULL);
    RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.customer_checkout_history(origin text,provider uuid,binding bytea,token bytea,
    target uuid,operation uuid,before_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; boundary timestamptz;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,false);
    IF ss.verifier IS NULL THEN RETURN NULL; END IF;
    IF before_id IS NOT NULL THEN
        SELECT i.created_at INTO boundary FROM relay_hosted.payment_intents i
            WHERE i.workspace_id=ss.workspace_id AND i.id=before_id AND i.checkout_method_id IS NOT NULL;
        IF NOT FOUND THEN RETURN '[]'; END IF;
    END IF;
    RETURN (SELECT coalesce(jsonb_agg(p.item ORDER BY p.created_at DESC,p.id DESC),'[]')
        FROM (SELECT i.id,i.created_at,relay_hosted.checkout_summary(i,m.label,j.id) item
            FROM relay_hosted.payment_intents i JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
            LEFT JOIN relay_hosted.journal_transactions j ON j.payment_id=i.id
            WHERE i.workspace_id=ss.workspace_id AND (target IS NULL OR i.id=target)
                AND (operation IS NULL OR i.operation_id=operation)
                AND (before_id IS NULL OR (i.created_at,i.id)<(boundary,before_id))
            ORDER BY i.created_at DESC,i.id DESC LIMIT 33) p);
END $$;

CREATE FUNCTION relay_hosted.prune_checkout_quotes() RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    DELETE FROM relay_hosted.checkout_quotes WHERE id IN (SELECT id FROM relay_hosted.checkout_quotes
        WHERE expires_at<=statement_timestamp() ORDER BY expires_at,id FOR UPDATE SKIP LOCKED LIMIT 256)
$$;

-- Private worker functions. The intent's globally unique ID, not the browser's
-- wallet-scoped operation ID, is the merchant's idempotency and lookup reference.
CREATE FUNCTION relay_hosted.claim_checkout_invoice(binding uuid)
RETURNS TABLE(id uuid,method_code text,currency text,payable_minor bigint)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH claimed AS (
        UPDATE relay_hosted.payment_intents i SET checkout_claimed_at=clock_timestamp()
        WHERE i.id=(SELECT x.id FROM relay_hosted.payment_intents x WHERE x.merchant_binding_id=binding
            AND x.checkout_method_id IS NOT NULL AND x.checkout_claimed_at IS NULL
            ORDER BY x.created_at,x.id FOR UPDATE SKIP LOCKED LIMIT 1)
        RETURNING i.*
    )
    SELECT c.id,m.method_code::text,c.quoted_currency,c.quoted_amount_minor
    FROM claimed c JOIN relay_hosted.checkout_methods m ON m.id=c.checkout_method_id
$$;

CREATE FUNCTION relay_hosted.checkout_invoice_for_lookup(binding uuid,target uuid)
RETURNS TABLE(id uuid,method_code text,currency text,payable_minor bigint)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT i.id,m.method_code::text,i.quoted_currency,i.quoted_amount_minor
    FROM relay_hosted.payment_intents i JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
    WHERE i.id=target AND i.merchant_binding_id=binding AND i.checkout_claimed_at IS NOT NULL
$$;

CREATE FUNCTION relay_hosted.record_checkout_invoice(binding uuid,target uuid,reference text,currency text,
    payable bigint,expiry bigint,closed boolean)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF reference IS NULL AND currency IS NULL AND payable IS NULL AND expiry IS NULL AND closed IS TRUE THEN
        UPDATE relay_hosted.payment_intents i SET invoice_closed_at=coalesce(i.invoice_closed_at,clock_timestamp())
            WHERE i.id=target AND i.merchant_binding_id=binding AND i.checkout_claimed_at IS NOT NULL
            AND i.merchant_reference IS NULL;
    ELSIF expiry BETWEEN 1 AND 253402300799 AND closed IS NOT NULL THEN
        UPDATE relay_hosted.payment_intents i SET merchant_reference=reference,invoice_expires_at=to_timestamp(expiry),
            invoice_closed_at=coalesce(i.invoice_closed_at,CASE WHEN closed THEN clock_timestamp() END)
            WHERE i.id=target AND i.merchant_binding_id=binding AND i.checkout_claimed_at IS NOT NULL
            AND i.quoted_currency=currency COLLATE "C" AND i.quoted_amount_minor=payable
            AND reference IS NOT NULL AND to_timestamp(expiry)>i.created_at
            AND ((i.merchant_reference IS NULL AND i.invoice_closed_at IS NULL)
                OR (i.merchant_reference=reference COLLATE "C" AND i.invoice_expires_at=to_timestamp(expiry)));
    ELSE RETURN false;
    END IF;
    RETURN FOUND;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.local_checkout(text),
    relay_hosted.customer_checkout_methods(text,uuid,bytea,bytea),
    relay_hosted.quote_checkout(text,uuid,bytea,bytea,uuid,bigint),
    relay_hosted.checkout_summary(relay_hosted.payment_intents,text,uuid),
    relay_hosted.confirm_checkout(text,uuid,bytea,bytea,uuid,uuid),
    relay_hosted.customer_checkout_history(text,uuid,bytea,bytea,uuid,uuid,uuid),
    relay_hosted.prune_checkout_quotes(),relay_hosted.claim_checkout_invoice(uuid),
    relay_hosted.checkout_invoice_for_lookup(uuid,uuid),
    relay_hosted.record_checkout_invoice(uuid,uuid,text,text,bigint,bigint,boolean) FROM PUBLIC;
