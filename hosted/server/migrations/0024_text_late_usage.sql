-- Full usage prices the same counts at two independent, immutable tariffs.
CREATE FUNCTION relay_hosted.text_final_value(inputs bigint,cached bigint,outputs bigint,input_rate numeric,
    cached_rate numeric,output_rate numeric)
RETURNS numeric LANGUAGE sql IMMUTABLE SET search_path=pg_catalog AS $$
    SELECT CASE WHEN inputs>=0 AND outputs>=0 AND (cached BETWEEN 0 AND inputs
        OR (cached IS NULL AND (inputs=0 OR input_rate=cached_rate))) THEN
        ceil((inputs-coalesce(cached,0))*input_rate+coalesce(cached,0)*cached_rate+outputs*output_rate) END
$$;

CREATE OR REPLACE FUNCTION relay_hosted.value_text_purchase(oid uuid,aid uuid,inputs bigint,cached bigint,outputs bigint)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE owed numeric; ceiling bigint;
BEGIN
    SELECT relay_hosted.text_final_value(inputs,cached,outputs,p.input_rate,p.cached_rate,p.output_rate),a.bound_micro_usd
        INTO owed,ceiling FROM relay_hosted.text_purchase_attempts a JOIN relay_hosted.text_purchase_rates p
        ON (p.item,p.revision)=(a.item,a.rate_revision) WHERE a.attempt_id=aid;
    IF owed IS NULL THEN RETURN; END IF;
    INSERT INTO relay_hosted.text_purchase_results(attempt_id,input_tokens,cached_tokens,output_tokens,valued_micro_usd)
        VALUES(aid,inputs,cached,outputs,owed) ON CONFLICT (attempt_id) DO NOTHING;
    IF FOUND AND owed>ceiling THEN PERFORM relay_hosted.fence_text_route(oid,aid); END IF;
END $$;

CREATE TABLE relay_hosted.text_late_usage (
    request_id uuid PRIMARY KEY REFERENCES relay_hosted.text_requests,
    input_tokens bigint NOT NULL CHECK (input_tokens>=0),
    cached_tokens bigint CHECK (cached_tokens BETWEEN 0 AND input_tokens),
    output_tokens bigint NOT NULL CHECK (output_tokens>=0),
    valued_micro_usd numeric(40,0) NOT NULL CHECK (valued_micro_usd>=0),
    observed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER sealed_text_late_usage BEFORE UPDATE OR DELETE ON relay_hosted.text_late_usage
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

-- Recover exact counts already retained for a supplier on earlier installations.
INSERT INTO relay_hosted.text_late_usage(request_id,input_tokens,cached_tokens,output_tokens,valued_micro_usd,observed_at)
    SELECT r.id,p.input_tokens,p.cached_tokens,p.output_tokens,v.amount,p.recorded_at
    FROM relay_hosted.text_requests r JOIN relay_hosted.text_offers o ON o.id=r.offer_id
    JOIN relay_hosted.text_purchase_results p USING(attempt_id)
    CROSS JOIN LATERAL (SELECT relay_hosted.text_final_value(p.input_tokens,p.cached_tokens,p.output_tokens,
        o.input_rate,o.cached_rate,o.output_rate) amount) v
    WHERE r.basis IN ('provider_partial','policy_waiver') AND v.amount IS NOT NULL;

CREATE VIEW relay_hosted.text_customer_credits AS
    SELECT r.id AS request_id,r.workspace_id,greatest(r.charged-l.valued_micro_usd,0)::bigint AS amount_micro_usd,
        l.observed_at
    FROM relay_hosted.text_late_usage l JOIN relay_hosted.text_requests r ON r.id=l.request_id
    WHERE r.basis='provider_partial' AND r.charged>l.valued_micro_usd;

ALTER TABLE relay_hosted.journal_transactions DROP CONSTRAINT journal_transactions_check,
    ADD CONSTRAINT journal_transaction_subject CHECK (
        (operation_kind='payment_credit' AND payment_id IS NOT NULL AND request_id IS NULL)
        OR (operation_kind IN ('request_hold','request_close','request_credit') AND payment_id IS NULL AND request_id IS NOT NULL)
    );

CREATE OR REPLACE FUNCTION relay_hosted.request_lines(rid uuid,kind text)
RETURNS TABLE(seq smallint,account uuid,amount bigint) LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT e.seq,a.id,e.amount FROM relay_hosted.text_requests r
    LEFT JOIN relay_hosted.text_customer_credits c ON c.request_id=r.id AND kind='request_credit'
    CROSS JOIN LATERAL (VALUES
        (1::smallint,r.workspace_id,'customer_available',CASE kind WHEN 'request_hold' THEN -r.held
            WHEN 'request_close' THEN r.held-r.charged WHEN 'request_credit' THEN c.amount_micro_usd END),
        (2::smallint,r.workspace_id,'customer_reserved',CASE kind WHEN 'request_hold' THEN r.held WHEN 'request_close' THEN -r.held END),
        (3::smallint,NULL::uuid,'inference_revenue',CASE kind WHEN 'request_close' THEN r.charged
            WHEN 'request_credit' THEN -c.amount_micro_usd END)
    ) e(seq,workspace,kind,amount) JOIN relay_hosted.ledger_accounts a ON a.account_kind=e.kind AND a.workspace_id IS NOT DISTINCT FROM e.workspace
    WHERE r.id=rid AND e.amount<>0
$$;

CREATE OR REPLACE FUNCTION relay_hosted.check_request_postings()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j relay_hosted.journal_transactions; jid uuid;
BEGIN
    IF TG_TABLE_NAME='journal_transactions' THEN jid:=NEW.id; ELSE jid:=NEW.transaction_id; END IF;
    SELECT * INTO STRICT j FROM relay_hosted.journal_transactions WHERE id=jid;
    IF j.operation_kind='payment_credit' THEN
        IF EXISTS (SELECT FROM relay_hosted.journal_postings WHERE transaction_id=jid AND sequence>2) THEN
            RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='extra payment posting'; END IF;
    ELSIF (j.operation_kind='request_credit' AND NOT EXISTS (
        SELECT FROM relay_hosted.text_customer_credits c WHERE c.request_id=j.request_id
    )) OR EXISTS (SELECT FROM relay_hosted.request_lines(j.request_id,j.operation_kind) e
        FULL JOIN (SELECT * FROM relay_hosted.journal_postings WHERE transaction_id=jid) a
        ON (e.seq,e.account,e.amount)=(a.sequence,a.account_id,a.amount_micro_usd)
        WHERE e.seq IS NULL OR a.sequence IS NULL) THEN
        RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='invalid request postings';
    END IF;
    RETURN NULL;
END $$;

CREATE FUNCTION relay_hosted.record_late_text_usage(r relay_hosted.text_requests,inputs bigint,cached bigint,outputs bigint,
    amount numeric)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
    IF amount IS NULL OR r.closed_at IS NULL OR r.claimed_at IS NULL
        OR r.basis NOT IN ('provider_partial','policy_waiver') THEN RETURN; END IF;
    INSERT INTO relay_hosted.text_late_usage(request_id,input_tokens,cached_tokens,output_tokens,valued_micro_usd)
        VALUES(r.id,inputs,cached,outputs,amount) ON CONFLICT (request_id) DO NOTHING;
    IF FOUND AND (amount>r.held OR inputs>r.input_bound OR outputs>r.output_bound) THEN
        PERFORM relay_hosted.fence_text_route(r.offer_id,r.attempt_id);
    END IF;
END $$;

-- The caller already holds the wallet lock. Overflow must not discard evidence.
CREATE FUNCTION relay_hosted.post_text_credit(r relay_hosted.text_requests)
RETURNS void LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE credit bigint;
BEGIN
    IF r.basis IS DISTINCT FROM 'provider_partial' OR r.charged<=0 THEN RETURN; END IF;
    SELECT c.amount_micro_usd INTO credit FROM relay_hosted.text_customer_credits c
    WHERE c.request_id=r.id AND NOT EXISTS (SELECT FROM relay_hosted.journal_transactions j
        WHERE j.request_id=r.id AND j.operation_kind='request_credit');
    IF NOT FOUND THEN RETURN; END IF;
    UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd+credit,revision=w.revision+1
        WHERE w.workspace_id=r.workspace_id AND w.available_micro_usd<=9223372036854775807-w.reserved_micro_usd-credit;
    IF FOUND THEN PERFORM relay_hosted.record_request_journal(r,'request_credit'); END IF;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.finish_text(oid uuid,rid uuid,aid uuid,outcome text,inputs bigint,cached bigint,outputs bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.text_requests; o relay_hosted.text_offers; incoming_value numeric;
    in_time boolean; priceable boolean;
BEGIN
    IF outcome NOT IN ('completed','incomplete','unknown','not_sent','deadline') OR outcome IS NULL THEN RETURN 'invalid'; END IF;
    PERFORM FROM relay_hosted.wallets w JOIN relay_hosted.text_requests x ON x.workspace_id=w.workspace_id
        WHERE x.id=rid AND x.attempt_id=aid AND x.offer_id=oid FOR NO KEY UPDATE OF w;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=oid;
    SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid FOR UPDATE;
    in_time:=r.close_by>clock_timestamp();
    IF r.claimed_at IS NOT NULL AND outcome IN ('completed','incomplete') THEN
        IF inputs<0 OR outputs<0 OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs))
            THEN RETURN 'invalid'; END IF;
        incoming_value:=relay_hosted.text_final_value(inputs,cached,outputs,o.input_rate,o.cached_rate,o.output_rate);
        PERFORM relay_hosted.value_text_purchase(oid,aid,inputs,cached,outputs);
    END IF;
    IF r.closed_at IS NOT NULL THEN
        IF incoming_value IS NOT NULL THEN
            PERFORM relay_hosted.record_late_text_usage(r,inputs,cached,outputs,incoming_value);
        END IF;
        PERFORM relay_hosted.post_text_credit(r);
        RETURN r.accounting;
    END IF;
    IF r.execution='reserved' THEN
        IF outcome='not_sent' OR (outcome='deadline' AND r.claim_by<=clock_timestamp()) THEN
            PERFORM relay_hosted.close_text(r,'failed','not_sent'); RETURN 'released'; END IF;
        RETURN 'held';
    END IF;
    IF outcome='deadline' AND in_time THEN RETURN r.accounting; END IF;
    IF in_time AND outcome IN ('completed','incomplete')
        AND (r.execution IN ('dispatched','uncertain') OR incoming_value IS NOT NULL) THEN
        -- Full evidence can supersede a partial observation while the hold is still open.
        UPDATE relay_hosted.text_requests SET execution=CASE WHEN r.execution IN ('completed','incomplete')
            THEN r.execution ELSE outcome END,input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
            WHERE id=rid RETURNING * INTO r;
    ELSIF r.execution='dispatched' THEN
        UPDATE relay_hosted.text_requests SET execution='uncertain',accounting='pending' WHERE id=rid RETURNING * INTO r;
    END IF;
    IF relay_hosted.text_final_value(r.input_tokens,r.cached_tokens,r.output_tokens,o.input_rate,o.cached_rate,o.output_rate)
        IS NOT NULL THEN
        PERFORM relay_hosted.close_text(r,r.execution,'provider_final');
        RETURN 'settled';
    END IF;
    IF NOT in_time THEN
        priceable:=r.output_tokens IS NOT NULL OR (r.input_tokens IS NOT NULL
            AND (r.input_tokens=0 OR o.input_rate=o.cached_rate OR r.cached_tokens IS NOT NULL));
        PERFORM relay_hosted.close_text(r,r.execution,CASE WHEN priceable THEN 'provider_partial' ELSE 'policy_waiver' END);
        IF incoming_value IS NOT NULL THEN
            SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid;
            PERFORM relay_hosted.record_late_text_usage(r,inputs,cached,outputs,incoming_value);
            PERFORM relay_hosted.post_text_credit(r);
        END IF;
        RETURN CASE WHEN priceable THEN 'settled' ELSE 'released' END;
    END IF;
    RETURN 'pending';
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.due_text()
RETURNS TABLE(oid uuid,rid uuid,aid uuid) LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT q.offer_id,q.id,q.attempt_id FROM (
        SELECT r.offer_id,r.id,r.attempt_id,r.close_by AS due_at FROM relay_hosted.text_requests r
        WHERE r.closed_at IS NULL AND
            ((r.execution='reserved' AND r.claim_by<=statement_timestamp()) OR r.close_by<=statement_timestamp())
        UNION ALL
        SELECT r.offer_id,r.id,r.attempt_id,c.observed_at FROM relay_hosted.text_customer_credits c
        JOIN relay_hosted.text_requests r ON r.id=c.request_id JOIN relay_hosted.wallets w ON w.workspace_id=r.workspace_id
        WHERE w.available_micro_usd<=9223372036854775807-w.reserved_micro_usd-c.amount_micro_usd AND NOT EXISTS (
            SELECT FROM relay_hosted.journal_transactions j WHERE j.request_id=r.id AND j.operation_kind='request_credit')
    ) q ORDER BY q.due_at,q.id LIMIT 32
$$;

CREATE OR REPLACE FUNCTION relay_hosted.text_receipt(r relay_hosted.text_requests)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('request_id',r.id,'key_id',r.key_id,'model',o.model,'tariff_revision',o.revision::text,
        'started_at_s',floor(extract(epoch FROM r.started_at))::bigint,'execution',r.execution,'accounting',r.accounting,'basis',r.basis,
        'input_tokens',r.input_tokens::text,'cached_tokens',r.cached_tokens::text,'output_tokens',r.output_tokens::text,
        'total_tokens',(r.input_tokens::numeric+r.output_tokens)::text,
        'currency','USD','precision',6,'hold_micro_usd',r.held::text,'charge_micro_usd',r.charged::text,
        'wallet_receipt_id',(SELECT j.id FROM relay_hosted.journal_transactions j WHERE j.request_id=r.id AND j.operation_kind='request_close'),
        'credit',(SELECT jsonb_build_object('wallet_receipt_id',j.id,'amount_micro_usd',c.amount_micro_usd::text)
            FROM relay_hosted.text_customer_credits c JOIN relay_hosted.journal_transactions j USING(request_id)
            WHERE c.request_id=r.id AND j.operation_kind='request_credit'))
    FROM relay_hosted.text_offers o WHERE o.id=r.offer_id
$$;

CREATE OR REPLACE FUNCTION relay_hosted.customer_history(origin text,provider uuid,binding bytea,token bytea,kind text,
    hours integer,before_id uuid,target uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; anchor timestamptz:=statement_timestamp(); since_at timestamptz; cursor_at timestamptz; rows jsonb;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF hours NOT IN (24,168,720) OR hours IS NULL THEN RETURN NULL; END IF;
    since_at:=anchor-hours*interval '1 hour';
    IF kind='usage' THEN
        IF before_id IS NOT NULL THEN
            SELECT started_at INTO cursor_at FROM relay_hosted.text_requests WHERE workspace_id=space AND id=before_id;
            IF NOT FOUND THEN RETURN '[]'::jsonb; END IF;
        END IF;
        SELECT coalesce(jsonb_agg(p.receipt ORDER BY p.ts DESC,p.id DESC),'[]') INTO rows FROM
            (SELECT r.started_at AS ts,r.id,relay_hosted.text_receipt(r) AS receipt FROM relay_hosted.text_requests r
            WHERE r.workspace_id=space AND (target IS NULL OR r.id=target)
                AND (target IS NOT NULL OR (r.started_at>=since_at AND r.started_at<=anchor))
                AND (before_id IS NULL OR (r.started_at,r.id)<(cursor_at,before_id))
            ORDER BY r.started_at DESC,r.id DESC LIMIT 33) p;
    ELSIF kind='wallet' THEN
        IF before_id IS NOT NULL THEN
            SELECT created_at INTO cursor_at FROM relay_hosted.journal_transactions WHERE workspace_id=space AND id=before_id;
            IF NOT FOUND THEN RETURN '[]'::jsonb; END IF;
        END IF;
        SELECT coalesce(jsonb_agg(p.entry ORDER BY p.ts DESC,p.id DESC),'[]') INTO rows FROM
            (SELECT j.created_at AS ts,j.id,jsonb_build_object('id',j.id,'kind',j.operation_kind,'request_id',j.request_id,
                'posted_at_s',floor(extract(epoch FROM j.created_at))::bigint,'amount_micro_usd',CASE j.operation_kind
                    WHEN 'payment_credit' THEN i.credit_micro_usd WHEN 'request_hold' THEN r.held
                    WHEN 'request_credit' THEN c.amount_micro_usd ELSE r.charged END::text) AS entry
            FROM relay_hosted.journal_transactions j LEFT JOIN relay_hosted.payment_intents i ON i.id=j.payment_id
            LEFT JOIN relay_hosted.text_requests r ON r.id=j.request_id
            LEFT JOIN relay_hosted.text_customer_credits c ON c.request_id=j.request_id AND j.operation_kind='request_credit'
            WHERE j.workspace_id=space AND (target IS NULL OR j.id=target)
                AND (target IS NOT NULL OR (j.created_at>=since_at AND j.created_at<=anchor))
                AND (before_id IS NULL OR (j.created_at,j.id)<(cursor_at,before_id))
            ORDER BY j.created_at DESC,j.id DESC LIMIT 33) p;
    END IF;
    RETURN rows;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.text_final_value(bigint,bigint,bigint,numeric,numeric,numeric),
    relay_hosted.record_late_text_usage(relay_hosted.text_requests,bigint,bigint,bigint,numeric),
    relay_hosted.post_text_credit(relay_hosted.text_requests) FROM PUBLIC;
