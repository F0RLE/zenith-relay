CREATE TABLE relay_hosted.payment_refunds (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL,
    payment_id uuid NOT NULL,
    merchant_binding_id uuid NOT NULL,
    request_operation uuid NOT NULL,
    amount_micro_usd bigint NOT NULL CHECK (amount_micro_usd>0 AND amount_micro_usd%10000=0),
    requested_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    state text NOT NULL DEFAULT 'requested'
        CHECK (state IN ('requested','declined','queued','reconciling','succeeded','failed')),
    claimed_at timestamptz,
    resolved_at timestamptz,
    provider_reference relay_hosted.payment_reference,
    UNIQUE(workspace_id,id),
    UNIQUE(workspace_id,request_operation),
    UNIQUE(merchant_binding_id,provider_reference),
    FOREIGN KEY(workspace_id,payment_id) REFERENCES relay_hosted.payment_intents(workspace_id,id),
    FOREIGN KEY(merchant_binding_id,payment_id) REFERENCES relay_hosted.payment_intents(merchant_binding_id,id),
    FOREIGN KEY(payment_id) REFERENCES relay_hosted.journal_transactions(payment_id),
    CHECK ((claimed_at IS NOT NULL)=(state IN ('reconciling','succeeded','failed'))),
    CHECK ((resolved_at IS NOT NULL)=(state IN ('declined','succeeded','failed'))),
    CHECK ((provider_reference IS NOT NULL)=(state IN ('succeeded','failed')))
);
CREATE INDEX refund_payment_history ON relay_hosted.payment_refunds(payment_id,requested_at DESC,id DESC);
CREATE INDEX refund_case_history ON relay_hosted.payment_refunds(requested_at DESC,id DESC);
CREATE UNIQUE INDEX one_open_refund_per_payment ON relay_hosted.payment_refunds(payment_id)
    WHERE state IN ('requested','queued','reconciling');
CREATE INDEX refund_send_queue ON relay_hosted.payment_refunds(merchant_binding_id,requested_at,id) WHERE state='queued';
CREATE INDEX refund_lookup_queue ON relay_hosted.payment_refunds(merchant_binding_id,id) WHERE state='reconciling';
CREATE TRIGGER fixed_refund_terms BEFORE UPDATE OF id,workspace_id,payment_id,merchant_binding_id,request_operation,
    amount_micro_usd,requested_at OR DELETE ON relay_hosted.payment_refunds
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER sealed_refund_case BEFORE UPDATE OR DELETE ON relay_hosted.payment_refunds
    FOR EACH ROW WHEN (OLD.resolved_at IS NOT NULL) EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE TABLE relay_hosted.payment_refund_reviews (
    refund_id uuid PRIMARY KEY REFERENCES relay_hosted.payment_refunds,
    operation_id uuid NOT NULL UNIQUE,
    actor_id uuid NOT NULL REFERENCES relay_hosted.users,
    action text NOT NULL CHECK (action IN ('approve','decline')),
    reason_code text NOT NULL,
    reviewed_at timestamptz NOT NULL,
    CHECK ((action,reason_code) IN (('approve','customer_requested'),('decline','owner_declined')))
);
CREATE TRIGGER sealed_refund_review BEFORE UPDATE OR DELETE ON relay_hosted.payment_refund_reviews
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

ALTER TABLE relay_hosted.wallets ADD COLUMN refund_reserved_micro_usd bigint NOT NULL DEFAULT 0
    CHECK (refund_reserved_micro_usd BETWEEN 0 AND reserved_micro_usd);
ALTER TABLE relay_hosted.ledger_accounts DROP CONSTRAINT ledger_accounts_check,
    ADD CHECK ((workspace_id IS NOT NULL AND account_kind IN ('customer_available','customer_reserved','customer_refund_reserved'))
        OR (workspace_id IS NULL AND account_kind IN ('payment_clearing','inference_revenue')));
INSERT INTO relay_hosted.ledger_accounts(workspace_id,account_kind)
    SELECT workspace_id,'customer_refund_reserved' FROM relay_hosted.wallets;
CREATE OR REPLACE FUNCTION relay_hosted.init_personal_wallet()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    INSERT INTO relay_hosted.wallets(workspace_id) VALUES(NEW.id);
    INSERT INTO relay_hosted.ledger_accounts(workspace_id,account_kind)
        VALUES(NEW.id,'customer_available'),(NEW.id,'customer_reserved'),(NEW.id,'customer_refund_reserved');
    RETURN NULL;
END $$;

ALTER TABLE relay_hosted.journal_transactions DROP CONSTRAINT journal_transaction_subject,
    ADD COLUMN refund_id uuid,
    ADD FOREIGN KEY (workspace_id,refund_id) REFERENCES relay_hosted.payment_refunds(workspace_id,id),
    ADD UNIQUE (refund_id,operation_kind),
    ADD CONSTRAINT journal_transaction_subject CHECK (
        (operation_kind='payment_credit' AND payment_id IS NOT NULL AND request_id IS NULL AND refund_id IS NULL)
        OR (operation_kind IN ('request_hold','request_close','request_credit') AND payment_id IS NULL
            AND request_id IS NOT NULL AND refund_id IS NULL)
        OR (operation_kind IN ('refund_hold','refund_paid','refund_release') AND payment_id IS NULL
            AND request_id IS NULL AND refund_id IS NOT NULL)
    );
CREATE UNIQUE INDEX one_refund_close ON relay_hosted.journal_transactions(refund_id)
    WHERE operation_kind IN ('refund_paid','refund_release');

CREATE FUNCTION relay_hosted.refund_lines(target uuid,posting_kind text)
RETURNS TABLE(seq smallint,account uuid,amount bigint) LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT e.seq,a.id,e.amount FROM relay_hosted.payment_refunds r CROSS JOIN LATERAL (VALUES
        (1::smallint,r.workspace_id,'customer_refund_reserved',
            CASE WHEN posting_kind='refund_hold' THEN r.amount_micro_usd ELSE -r.amount_micro_usd END),
        (2::smallint,CASE WHEN posting_kind<>'refund_paid' THEN r.workspace_id END,
            CASE WHEN posting_kind='refund_paid' THEN 'payment_clearing' ELSE 'customer_available' END,
            CASE WHEN posting_kind='refund_hold' THEN -r.amount_micro_usd ELSE r.amount_micro_usd END)
    ) e(seq,workspace,kind,amount) JOIN relay_hosted.ledger_accounts a
        ON a.workspace_id IS NOT DISTINCT FROM e.workspace AND a.account_kind=e.kind
    WHERE r.id=target AND ((posting_kind='refund_hold' AND (r.state='queued' OR r.claimed_at IS NOT NULL))
        OR (posting_kind='refund_paid' AND r.state='succeeded')
        OR (posting_kind='refund_release' AND r.state='failed'))
$$;

CREATE FUNCTION relay_hosted.record_refund_journal(r relay_hosted.payment_refunds,kind text)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE receipt uuid;
BEGIN
    INSERT INTO relay_hosted.journal_transactions(workspace_id,operation_kind,refund_id)
        VALUES(r.workspace_id,kind,r.id) RETURNING id INTO receipt;
    INSERT INTO relay_hosted.journal_postings(transaction_id,sequence,account_id,amount_micro_usd)
        SELECT receipt,seq,account,amount FROM relay_hosted.refund_lines(r.id,kind);
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.check_request_postings()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j relay_hosted.journal_transactions; jid uuid;
BEGIN
    IF TG_TABLE_NAME='journal_transactions' THEN jid:=NEW.id; ELSE jid:=NEW.transaction_id; END IF;
    SELECT * INTO STRICT j FROM relay_hosted.journal_transactions WHERE id=jid;
    IF j.operation_kind='payment_credit' THEN
        IF EXISTS (SELECT FROM relay_hosted.journal_postings WHERE transaction_id=jid AND sequence>2) THEN
            RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='extra payment posting'; END IF;
    ELSIF j.refund_id IS NOT NULL THEN
        IF (SELECT count(*) FROM relay_hosted.journal_postings WHERE transaction_id=jid)<>2
            OR EXISTS (SELECT FROM relay_hosted.refund_lines(j.refund_id,j.operation_kind) e
                FULL JOIN (SELECT * FROM relay_hosted.journal_postings WHERE transaction_id=jid) a
                ON (e.seq,e.account,e.amount)=(a.sequence,a.account_id,a.amount_micro_usd)
                WHERE e.seq IS NULL OR a.sequence IS NULL) THEN
            RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='invalid refund postings'; END IF;
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

CREATE FUNCTION relay_hosted.refundable_credit(i relay_hosted.payment_intents)
RETURNS bigint LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT (i.credit_micro_usd-coalesce(sum(r.amount_micro_usd),0))::bigint
    FROM relay_hosted.payment_refunds r WHERE r.payment_id=i.id AND r.state IN ('queued','reconciling','succeeded')
$$;

CREATE FUNCTION relay_hosted.refund_summary(r relay_hosted.payment_refunds)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',r.id,'payment_id',r.payment_id,'state',r.state,
        'amount_micro_usd',r.amount_micro_usd::text,'requested_at_s',floor(extract(epoch FROM r.requested_at))::bigint,
        'receipt_id',(SELECT j.id FROM relay_hosted.journal_transactions j WHERE j.refund_id=r.id
            AND j.operation_kind IN ('refund_paid','refund_release')))
$$;

CREATE OR REPLACE FUNCTION relay_hosted.checkout_summary(i relay_hosted.payment_intents,label text,receipt uuid)
RETURNS jsonb LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',i.id,'operation_id',i.operation_id,'quote_id',i.quote_id,
        'label',label,'credit_micro_usd',i.credit_micro_usd::text,'currency',i.quoted_currency,
        'payable_minor',i.quoted_amount_minor::text,'receipt_id',receipt,
        'created_at_s',floor(extract(epoch FROM i.created_at))::bigint,
        'state',CASE WHEN receipt IS NOT NULL THEN 'paid' WHEN i.invoice_closed_at IS NOT NULL THEN 'closed'
            WHEN i.invoice_resume_state<>'uncertain' AND i.invoice_expires_at>statement_timestamp()
                THEN 'awaiting_payment' ELSE 'processing' END,
        'can_resume',coalesce(i.invoice_resume_state='ready' AND receipt IS NULL AND i.invoice_closed_at IS NULL
            AND i.invoice_expires_at>statement_timestamp(),false),
        'refundable_micro_usd',CASE WHEN receipt IS NOT NULL AND i.checkout_method_id IS NOT NULL
            AND i.merchant_reference IS NOT NULL THEN relay_hosted.refundable_credit(i)::text ELSE '0' END,
        'refund',(SELECT relay_hosted.refund_summary(r) FROM relay_hosted.payment_refunds r
            WHERE r.payment_id=i.id ORDER BY r.requested_at DESC,r.id DESC LIMIT 1))
$$;

CREATE FUNCTION relay_hosted.customer_wallet(origin text,provider uuid,binding bytea,token bytea)
RETURNS TABLE(workspace_id uuid,available bigint,reserved bigint,refund_reserved bigint)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN; END IF;
    RETURN QUERY SELECT w.workspace_id,w.available_micro_usd,w.reserved_micro_usd,w.refund_reserved_micro_usd
        FROM relay_hosted.wallets w WHERE w.workspace_id=space;
END $$;

CREATE FUNCTION relay_hosted.request_payment_refund(origin text,provider uuid,binding bytea,token bytea,
    target uuid,op uuid,amount bigint)
RETURNS TABLE(outcome text,payload jsonb) LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; i relay_hosted.payment_intents; r relay_hosted.payment_refunds;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    IF NOT relay_hosted.local_checkout(origin) THEN outcome:='unavailable'; RETURN NEXT; RETURN; END IF;
    IF op='00000000-0000-0000-0000-000000000000' OR amount<=0 OR amount%10000<>0
        THEN outcome:='amount'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO i FROM relay_hosted.payment_intents x WHERE x.workspace_id=ss.workspace_id AND x.id=target
        AND x.checkout_method_id IS NOT NULL;
    IF NOT FOUND THEN outcome:='missing'; RETURN NEXT; RETURN; END IF;
    -- Order the case with concurrent approvals, credit and refund settlement.
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=ss.workspace_id FOR NO KEY UPDATE;
    IF ss.expires_at<=clock_timestamp() OR NOT EXISTS (SELECT FROM relay_hosted.configuration_revisions c
        WHERE c.id=ss.scope_id AND c.expires_at>clock_timestamp())
        THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO r FROM relay_hosted.payment_refunds x WHERE x.workspace_id=ss.workspace_id AND x.request_operation=op;
    IF FOUND THEN
        IF (r.payment_id,r.amount_micro_usd)=(target,amount) THEN
            outcome:='accepted'; payload:=relay_hosted.refund_summary(r);
        ELSE outcome:='conflict'; END IF;
        RETURN NEXT; RETURN;
    END IF;
    -- Re-read the original immutable reference after taking the wallet lock.
    SELECT * INTO STRICT i FROM relay_hosted.payment_intents WHERE id=target;
    IF i.merchant_reference IS NULL OR NOT EXISTS (SELECT FROM relay_hosted.journal_transactions WHERE payment_id=target)
        THEN outcome:='conflict'; RETURN NEXT; RETURN; END IF;
    IF EXISTS (SELECT FROM relay_hosted.payment_refunds x WHERE x.payment_id=target
        AND x.state IN ('requested','queued','reconciling')) THEN outcome:='conflict'; RETURN NEXT; RETURN; END IF;
    IF amount>relay_hosted.refundable_credit(i) THEN outcome:='remainder'; RETURN NEXT; RETURN; END IF;
    INSERT INTO relay_hosted.payment_refunds(workspace_id,payment_id,merchant_binding_id,request_operation,amount_micro_usd)
        VALUES(ss.workspace_id,target,i.merchant_binding_id,op,amount) RETURNING * INTO r;
    outcome:='accepted'; payload:=relay_hosted.refund_summary(r); RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.owner_refund_cases(origin text,tag bytea,target uuid,before_id uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE boundary timestamptz;
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    IF before_id IS NOT NULL THEN
        SELECT r.requested_at INTO boundary FROM relay_hosted.payment_refunds r
            WHERE r.id=before_id AND (target IS NULL OR r.payment_id=target);
        IF NOT FOUND THEN RETURN '[]'; END IF;
    END IF;
    RETURN (SELECT coalesce(jsonb_agg(p.item ORDER BY p.requested_at DESC,p.id DESC),'[]')
        FROM (SELECT r.id,r.requested_at,relay_hosted.refund_summary(r) || jsonb_build_object('label',m.label) item
            FROM relay_hosted.payment_refunds r JOIN relay_hosted.payment_intents i ON i.id=r.payment_id
            JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
            WHERE (target IS NULL OR r.payment_id=target)
                AND (before_id IS NULL OR (r.requested_at,r.id)<(boundary,before_id))
            ORDER BY r.requested_at DESC,r.id DESC LIMIT 33) p);
END $$;

CREATE FUNCTION relay_hosted.review_payment_refund(origin text,tag bytea,target uuid,op uuid,expected text,verb text)
RETURNS text LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE actor uuid; ss relay_hosted.admin_sessions; r relay_hosted.payment_refunds;
    i relay_hosted.payment_intents; w relay_hosted.wallets; stamp timestamptz;
BEGIN
    SELECT sess.* INTO ss FROM relay_hosted.active_owner_credentials a
        JOIN relay_hosted.admin_sessions sess ON sess.user_id=a.user_id AND sess.credential_id=a.credential_id
        WHERE sess.verifier=tag AND a.public_origin=origin AND sess.installation_epoch=a.installation_epoch
            AND sess.user_epoch=a.user_epoch AND sess.expires_at>clock_timestamp()
            AND sess.last_seen_at>clock_timestamp()-interval '20 minutes' FOR SHARE OF sess,a;
    IF NOT FOUND THEN RETURN 'denied'; END IF;
    actor:=ss.user_id;
    IF op='00000000-0000-0000-0000-000000000000' OR expected<>'requested' OR verb NOT IN ('approve','decline')
        THEN RETURN 'invalid'; END IF;
    IF NOT relay_hosted.local_checkout(origin) THEN RETURN 'unavailable'; END IF;
    SELECT * INTO w FROM relay_hosted.wallets x
        WHERE x.workspace_id=(SELECT workspace_id FROM relay_hosted.payment_refunds WHERE id=target) FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT r FROM relay_hosted.payment_refunds WHERE id=target FOR UPDATE;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    IF EXISTS (SELECT FROM relay_hosted.payment_refund_reviews x WHERE x.operation_id=op) THEN
        IF EXISTS (SELECT FROM relay_hosted.payment_refund_reviews x WHERE x.operation_id=op
            AND (x.refund_id,x.actor_id,x.action)=(target,actor,verb)) THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF r.state<>expected THEN RETURN 'conflict'; END IF;
    IF ss.created_at<=stamp-interval '2 minutes' THEN RETURN 'fresh_required'; END IF;
    IF verb='approve' THEN
        SELECT * INTO STRICT i FROM relay_hosted.payment_intents WHERE id=r.payment_id;
        IF r.amount_micro_usd>relay_hosted.refundable_credit(i) THEN RETURN 'remainder'; END IF;
        IF r.amount_micro_usd>w.available_micro_usd THEN RETURN 'balance'; END IF;
    END IF;
    INSERT INTO relay_hosted.payment_refund_reviews(refund_id,operation_id,actor_id,action,reason_code,reviewed_at)
        VALUES(target,op,actor,verb,CASE verb WHEN 'approve' THEN 'customer_requested' ELSE 'owner_declined' END,stamp)
        ON CONFLICT DO NOTHING;
    IF NOT FOUND THEN RETURN 'conflict'; END IF;
    IF verb='decline' THEN
        UPDATE relay_hosted.payment_refunds SET state='declined',resolved_at=stamp WHERE id=target;
    ELSE
        UPDATE relay_hosted.wallets SET available_micro_usd=available_micro_usd-r.amount_micro_usd,
            reserved_micro_usd=reserved_micro_usd+r.amount_micro_usd,
            refund_reserved_micro_usd=refund_reserved_micro_usd+r.amount_micro_usd,revision=revision+1
            WHERE workspace_id=r.workspace_id;
        UPDATE relay_hosted.payment_refunds SET state='queued' WHERE id=target RETURNING * INTO STRICT r;
        PERFORM relay_hosted.record_refund_journal(r,'refund_hold');
    END IF;
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.claim_payment_refund(binding uuid)
RETURNS TABLE(id uuid,invoice_reference text,currency text,amount_minor bigint,terminal boolean)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH claimed AS (
        UPDATE relay_hosted.payment_refunds r SET state='reconciling',claimed_at=clock_timestamp()
        WHERE r.id=(SELECT x.id FROM relay_hosted.payment_refunds x WHERE x.merchant_binding_id=binding
            AND x.state='queued' ORDER BY x.requested_at,x.id LIMIT 1 FOR UPDATE SKIP LOCKED)
        RETURNING r.*
    )
    SELECT r.id,i.merchant_reference::text,i.quoted_currency,r.amount_micro_usd/10000,false
    FROM claimed r JOIN relay_hosted.payment_intents i ON i.id=r.payment_id
$$;

CREATE FUNCTION relay_hosted.pending_refund_lookups(binding uuid,after_id uuid)
RETURNS SETOF uuid LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT r.id FROM relay_hosted.payment_refunds r WHERE r.merchant_binding_id=binding AND r.state='reconciling'
        AND (after_id IS NULL OR r.id>after_id) ORDER BY r.id LIMIT 32
$$;

CREATE FUNCTION relay_hosted.refund_for_lookup(binding uuid,target uuid)
RETURNS TABLE(id uuid,invoice_reference text,currency text,amount_minor bigint,terminal boolean)
LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT r.id,i.merchant_reference::text,i.quoted_currency,r.amount_micro_usd/10000,r.resolved_at IS NOT NULL
    FROM relay_hosted.payment_refunds r JOIN relay_hosted.payment_intents i ON i.id=r.payment_id
    WHERE r.id=target AND r.merchant_binding_id=binding AND r.claimed_at IS NOT NULL
$$;

CREATE FUNCTION relay_hosted.observe_payment_refund(binding uuid,target uuid,invoice_ref text,currency text,minor bigint,
    refund_ref text,succeeded boolean)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.payment_refunds; terminal_state text:=CASE WHEN succeeded THEN 'succeeded' ELSE 'failed' END;
BEGIN
    -- Approval and inference hold/close take the wallet lock first as well.
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=(SELECT x.workspace_id FROM relay_hosted.payment_refunds x
        WHERE x.id=target AND x.merchant_binding_id=binding) FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    SELECT * INTO STRICT r FROM relay_hosted.payment_refunds WHERE id=target FOR UPDATE;
    IF r.claimed_at IS NULL OR r.amount_micro_usd/10000<>minor OR NOT EXISTS (
        SELECT FROM relay_hosted.payment_intents i WHERE i.id=r.payment_id
            AND i.merchant_reference=invoice_ref COLLATE "C" AND i.quoted_currency=currency COLLATE "C"
    ) THEN RETURN false; END IF;
    IF r.resolved_at IS NOT NULL THEN RETURN r.state=terminal_state AND r.provider_reference=refund_ref COLLATE "C"; END IF;
    UPDATE relay_hosted.payment_refunds SET state=terminal_state,resolved_at=clock_timestamp(),
        provider_reference=refund_ref WHERE id=target RETURNING * INTO STRICT r;
    UPDATE relay_hosted.wallets SET
        available_micro_usd=available_micro_usd+CASE WHEN succeeded THEN 0 ELSE r.amount_micro_usd END,
        reserved_micro_usd=reserved_micro_usd-r.amount_micro_usd,
        refund_reserved_micro_usd=refund_reserved_micro_usd-r.amount_micro_usd,revision=revision+1
        WHERE workspace_id=r.workspace_id;
    PERFORM relay_hosted.record_refund_journal(r,CASE WHEN succeeded THEN 'refund_paid' ELSE 'refund_release' END);
    RETURN true;
END $$;

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
                'refund_id',j.refund_id,'posted_at_s',floor(extract(epoch FROM j.created_at))::bigint,
                'amount_micro_usd',coalesce(f.amount_micro_usd,CASE j.operation_kind
                    WHEN 'payment_credit' THEN i.credit_micro_usd WHEN 'request_hold' THEN r.held
                    WHEN 'request_credit' THEN c.amount_micro_usd ELSE r.charged END)::text) AS entry
            FROM relay_hosted.journal_transactions j LEFT JOIN relay_hosted.payment_intents i ON i.id=j.payment_id
            LEFT JOIN relay_hosted.text_requests r ON r.id=j.request_id
            LEFT JOIN relay_hosted.text_customer_credits c ON c.request_id=j.request_id AND j.operation_kind='request_credit'
            LEFT JOIN relay_hosted.payment_refunds f ON f.id=j.refund_id
            WHERE j.workspace_id=space AND (target IS NULL OR j.id=target)
                AND (target IS NOT NULL OR (j.created_at>=since_at AND j.created_at<=anchor))
                AND (before_id IS NULL OR (j.created_at,j.id)<(cursor_at,before_id))
            ORDER BY j.created_at DESC,j.id DESC LIMIT 33) p;
    END IF;
    RETURN rows;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.refund_lines(uuid,text),relay_hosted.record_refund_journal(relay_hosted.payment_refunds,text),
    relay_hosted.refundable_credit(relay_hosted.payment_intents),relay_hosted.refund_summary(relay_hosted.payment_refunds),
    relay_hosted.customer_wallet(text,uuid,bytea,bytea),relay_hosted.request_payment_refund(text,uuid,bytea,bytea,uuid,uuid,bigint),
    relay_hosted.owner_refund_cases(text,bytea,uuid,uuid),relay_hosted.review_payment_refund(text,bytea,uuid,uuid,text,text),
    relay_hosted.claim_payment_refund(uuid),relay_hosted.pending_refund_lookups(uuid,uuid),relay_hosted.refund_for_lookup(uuid,uuid),
    relay_hosted.observe_payment_refund(uuid,uuid,text,text,bigint,text,boolean) FROM PUBLIC;
