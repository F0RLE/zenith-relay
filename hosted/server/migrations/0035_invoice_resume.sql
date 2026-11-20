ALTER TABLE relay_hosted.payment_intents
    ADD COLUMN invoice_resume_state text NOT NULL DEFAULT 'unavailable'
        CHECK (invoice_resume_state IN ('unavailable','ready','uncertain')),
    ADD CHECK (invoice_resume_state<>'ready' OR (checkout_method_id IS NOT NULL AND merchant_reference IS NOT NULL));

-- Only the one-time exchange can hold a handoff. Nothing in the journal or
-- payment history stores it. Losing this table on database restart is safe.
CREATE UNLOGGED TABLE relay_hosted.checkout_resume_checks (
    payment_id uuid PRIMARY KEY REFERENCES relay_hosted.payment_intents,
    probe uuid NOT NULL UNIQUE,
    deadline timestamptz NOT NULL,
    claimed boolean NOT NULL DEFAULT false,
    finished boolean NOT NULL DEFAULT false,
    handoff text,
    CHECK (NOT finished OR claimed),
    CHECK (handoff IS NULL OR (finished AND octet_length(handoff)<=2048 AND handoff LIKE 'https://%'))
);
CREATE INDEX checkout_resume_deadline ON relay_hosted.checkout_resume_checks(deadline);

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
            AND i.invoice_expires_at>statement_timestamp(),false))
$$;

CREATE FUNCTION relay_hosted.observe_checkout_invoice(binding uuid,target uuid,reference text,currency text,
    payable bigint,expiry bigint,closed boolean,resumable boolean)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE uncertain boolean:=reference IS NULL AND currency IS NULL AND payable IS NULL AND expiry IS NULL
    AND closed IS FALSE AND resumable IS FALSE;
BEGIN
    IF NOT uncertain AND relay_hosted.record_checkout_invoice(binding,target,reference,currency,payable,expiry,closed) IS NOT TRUE
    THEN RETURN false; END IF;
    UPDATE relay_hosted.payment_intents i
        SET invoice_resume_state=CASE WHEN uncertain THEN 'uncertain'
            WHEN resumable IS TRUE AND i.merchant_reference IS NOT NULL AND i.invoice_closed_at IS NULL THEN 'ready'
            ELSE 'unavailable' END
        WHERE i.id=target AND i.merchant_binding_id=binding AND i.checkout_claimed_at IS NOT NULL;
    RETURN FOUND;
END $$;

CREATE FUNCTION relay_hosted.customer_checkout_resume(origin text,provider uuid,binding bytea,token bytea,
    target uuid,ticket uuid,enqueue boolean)
RETURNS TABLE(outcome text,payment jsonb,handoff text)
LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; check_row relay_hosted.checkout_resume_checks;
BEGIN
    ss:=relay_hosted.key_authority(origin,provider,binding,token,false);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT relay_hosted.checkout_summary(i,m.label,j.id) INTO payment
        FROM relay_hosted.payment_intents i JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
        LEFT JOIN relay_hosted.journal_transactions j ON j.payment_id=i.id
        WHERE i.workspace_id=ss.workspace_id AND i.id=target;
    IF NOT FOUND THEN outcome:='missing'; RETURN NEXT; RETURN; END IF;
    outcome:='ready';
    IF NOT relay_hosted.local_checkout(origin) OR NOT (payment->>'can_resume')::boolean THEN
        IF NOT enqueue THEN
            DELETE FROM relay_hosted.checkout_resume_checks c WHERE c.payment_id=target AND c.probe=ticket;
        END IF;
    ELSIF enqueue THEN
        INSERT INTO relay_hosted.checkout_resume_checks AS c(payment_id,probe,deadline)
            VALUES(target,ticket,clock_timestamp()+interval '6 seconds')
            ON CONFLICT (payment_id) DO UPDATE SET probe=excluded.probe,deadline=excluded.deadline,
                claimed=false,finished=false,handoff=NULL WHERE c.deadline<=clock_timestamp();
        IF FOUND THEN outcome:='waiting'; END IF;
    ELSE
        SELECT c.* INTO check_row FROM relay_hosted.checkout_resume_checks c
            WHERE c.payment_id=target AND c.probe=ticket;
        IF NOT FOUND THEN RETURN NEXT; RETURN; END IF;
        IF NOT check_row.finished AND check_row.deadline>clock_timestamp() THEN
            outcome:='waiting';
        ELSE
            IF check_row.handoff IS NOT NULL THEN
                -- Order delivery with concurrent credit. No merchant I/O under this lock.
                PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=ss.workspace_id FOR SHARE;
                IF (relay_hosted.key_authority(origin,provider,binding,token,false)).verifier IS NULL
                THEN outcome:='denied'; payment:=NULL; RETURN NEXT; RETURN; END IF;
            END IF;
            DELETE FROM relay_hosted.checkout_resume_checks c WHERE c.payment_id=target AND c.probe=ticket
                RETURNING CASE WHEN check_row.handoff IS NOT NULL AND c.deadline>clock_timestamp()
                    THEN c.handoff END INTO handoff;
            SELECT relay_hosted.checkout_summary(i,m.label,j.id) INTO payment
                FROM relay_hosted.payment_intents i JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
                LEFT JOIN relay_hosted.journal_transactions j ON j.payment_id=i.id
                WHERE i.id=target AND i.workspace_id=ss.workspace_id;
            IF NOT (payment->>'can_resume')::boolean OR NOT EXISTS (SELECT FROM relay_hosted.payment_intents i
                WHERE i.id=target AND i.invoice_expires_at>clock_timestamp())
            THEN handoff:=NULL; END IF;
        END IF;
    END IF;
    RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.claim_checkout_resume(binding uuid)
RETURNS TABLE(probe uuid,id uuid,method_code text,currency text,payable_minor bigint,invoice_reference text,expires_at_s bigint)
LANGUAGE sql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    WITH picked AS (
        SELECT c.payment_id,i.checkout_method_id,i.quoted_currency,i.quoted_amount_minor,
            i.merchant_reference,i.invoice_expires_at
        FROM relay_hosted.checkout_resume_checks c JOIN relay_hosted.payment_intents i ON i.id=c.payment_id
        WHERE i.merchant_binding_id=binding AND NOT c.claimed AND c.deadline>clock_timestamp()
            AND i.invoice_resume_state='ready' AND i.invoice_closed_at IS NULL AND i.invoice_expires_at>clock_timestamp()
            AND NOT EXISTS (SELECT FROM relay_hosted.journal_transactions j WHERE j.payment_id=i.id)
        ORDER BY c.deadline,c.payment_id LIMIT 1 FOR UPDATE OF c SKIP LOCKED
    ), claimed AS (
        UPDATE relay_hosted.checkout_resume_checks c SET claimed=true FROM picked p
            WHERE c.payment_id=p.payment_id RETURNING c.probe,p.*
    )
    SELECT c.probe,c.payment_id,m.method_code::text,c.quoted_currency,c.quoted_amount_minor,
        c.merchant_reference::text,extract(epoch FROM c.invoice_expires_at)::bigint
    FROM claimed c JOIN relay_hosted.checkout_methods m ON m.id=c.checkout_method_id
$$;

CREATE FUNCTION relay_hosted.finish_checkout_resume(binding uuid,target uuid,ticket uuid,destination text)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    UPDATE relay_hosted.checkout_resume_checks c SET finished=true,
        handoff=CASE WHEN i.invoice_resume_state='ready' AND i.invoice_closed_at IS NULL
            AND i.invoice_expires_at>clock_timestamp()
            AND NOT EXISTS (SELECT FROM relay_hosted.journal_transactions j WHERE j.payment_id=i.id)
            THEN destination END
        FROM relay_hosted.payment_intents i WHERE c.probe=ticket AND c.payment_id=i.id AND i.id=target
            AND i.merchant_binding_id=binding AND c.claimed AND NOT c.finished AND c.deadline>clock_timestamp();
    IF FOUND AND destination IS NULL THEN
        UPDATE relay_hosted.payment_intents i SET invoice_resume_state='uncertain'
            WHERE i.id=target AND i.merchant_binding_id=binding AND i.invoice_resume_state='ready';
    END IF;
END $$;

CREATE FUNCTION relay_hosted.prune_checkout_resume_checks() RETURNS void
LANGUAGE sql SECURITY DEFINER SET search_path=pg_catalog AS $$
    DELETE FROM relay_hosted.checkout_resume_checks WHERE payment_id IN (
        SELECT payment_id FROM relay_hosted.checkout_resume_checks
        WHERE deadline<=statement_timestamp() ORDER BY deadline LIMIT 256 FOR UPDATE SKIP LOCKED)
$$;

REVOKE ALL ON FUNCTION relay_hosted.observe_checkout_invoice(uuid,uuid,text,text,bigint,bigint,boolean,boolean),
    relay_hosted.customer_checkout_resume(text,uuid,bytea,bytea,uuid,uuid,boolean),
    relay_hosted.claim_checkout_resume(uuid),relay_hosted.finish_checkout_resume(uuid,uuid,uuid,text),
    relay_hosted.prune_checkout_resume_checks() FROM PUBLIC;
