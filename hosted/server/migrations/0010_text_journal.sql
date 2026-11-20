-- A reviewed immutable offer for one stateless native text route in one owner-approved scope.
-- The fixture supplies the evidence; customer login or model discovery never inserts an offer.
CREATE TABLE relay_hosted.text_offers (
    id uuid PRIMARY KEY,
    scope_id uuid NOT NULL REFERENCES relay_hosted.configuration_revisions,
    source_id uuid NOT NULL,
    provider_revision bigint NOT NULL CHECK (provider_revision>0),
    base_url text NOT NULL CHECK (octet_length(base_url) BETWEEN 1 AND 256),
    model text COLLATE "C" NOT NULL CHECK (model ~ '^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$'),
    revision bigint NOT NULL CHECK (revision>0),
    max_input_bytes integer NOT NULL CHECK (max_input_bytes BETWEEN 1 AND 60000),
    max_output_tokens integer NOT NULL CHECK (max_output_tokens BETWEEN 1 AND 1000000),
    tokens_per_byte integer NOT NULL CHECK (tokens_per_byte BETWEEN 1 AND 16),
    overhead_tokens integer NOT NULL CHECK (overhead_tokens BETWEEN 0 AND 65536),
    input_rate numeric(30,12) NOT NULL CHECK (input_rate>=0 AND input_rate<1e12),
    cached_rate numeric(30,12) NOT NULL CHECK (cached_rate>=0 AND cached_rate<1e12),
    output_rate numeric(30,12) NOT NULL CHECK (output_rate>=0 AND output_rate<1e12),
    close_after_s integer NOT NULL CHECK (close_after_s BETWEEN 65 AND 3600),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(scope_id,source_id,model,revision),
    UNIQUE(id,scope_id)
);
CREATE TRIGGER immutable_text_offer BEFORE UPDATE OR DELETE ON relay_hosted.text_offers
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TABLE relay_hosted.text_grants (
    offer_id uuid NOT NULL,
    scope_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    PRIMARY KEY(offer_id,workspace_id),
    FOREIGN KEY(offer_id,scope_id) REFERENCES relay_hosted.text_offers(id,scope_id),
    FOREIGN KEY(scope_id,workspace_id) REFERENCES relay_hosted.qualification_subjects(scope_id,workspace_id)
);
CREATE TABLE relay_hosted.text_fences (
    offer_id uuid PRIMARY KEY REFERENCES relay_hosted.text_offers,
    at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TABLE relay_hosted.text_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    attempt_id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES relay_hosted.wallets,
    key_id uuid NOT NULL,
    offer_id uuid NOT NULL REFERENCES relay_hosted.text_offers,
    authority_epoch bigint NOT NULL CHECK (authority_epoch>0),
    -- HMACs only. Never store an idempotency header, prompt, response or unkeyed prompt hash.
    operation_tag bytea CHECK (octet_length(operation_tag)=32),
    input_tag bytea NOT NULL CHECK (octet_length(input_tag)=32),
    send_tag bytea NOT NULL CHECK (octet_length(send_tag)=32),
    input_bound integer NOT NULL CHECK (input_bound>0),
    output_bound integer NOT NULL CHECK (output_bound>0),
    held bigint NOT NULL CHECK (held>=0),
    execution text NOT NULL DEFAULT 'reserved' CHECK (execution IN ('reserved','dispatched','completed','incomplete','failed','uncertain')),
    accounting text NOT NULL DEFAULT 'held' CHECK (accounting IN ('held','pending','settled','released')),
    basis text CHECK (basis IN ('provider_final','provider_partial','not_sent','policy_waiver')),
    input_tokens bigint CHECK (input_tokens>=0),
    cached_tokens bigint CONSTRAINT text_cache_within_input CHECK (
        cached_tokens IS NULL OR (input_tokens IS NOT NULL AND cached_tokens BETWEEN 0 AND input_tokens)),
    output_tokens bigint CHECK (output_tokens>=0),
    valued numeric(40,0) CHECK (valued>=0),
    charged bigint CHECK (charged BETWEEN 0 AND held),
    started_at timestamptz NOT NULL,
    claim_by timestamptz NOT NULL,
    close_by timestamptz NOT NULL CHECK (close_by>claim_by),
    claimed_at timestamptz,
    closed_at timestamptz,
    UNIQUE(workspace_id,id), UNIQUE(workspace_id,key_id,operation_tag),
    FOREIGN KEY(workspace_id,key_id) REFERENCES relay_hosted.customer_keys(workspace_id,id),
    CHECK ((closed_at IS NULL)=(charged IS NULL) AND (closed_at IS NULL)=(basis IS NULL)
        AND (closed_at IS NULL)=(accounting IN ('held','pending')))
);
CREATE TRIGGER immutable_text_admission BEFORE UPDATE OF id,attempt_id,workspace_id,key_id,offer_id,authority_epoch,
    operation_tag,input_tag,send_tag,input_bound,output_bound,held,started_at,claim_by,close_by ON relay_hosted.text_requests
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER sealed_text_receipt BEFORE UPDATE OR DELETE ON relay_hosted.text_requests
    FOR EACH ROW WHEN (OLD.closed_at IS NOT NULL) EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE INDEX text_history ON relay_hosted.text_requests(workspace_id,started_at DESC,id DESC);
CREATE INDEX text_pending ON relay_hosted.text_requests(offer_id,close_by) WHERE closed_at IS NULL;
CREATE INDEX text_waiting ON relay_hosted.text_requests(offer_id,claim_by) WHERE execution='reserved';

ALTER TABLE relay_hosted.ledger_accounts DROP CONSTRAINT ledger_accounts_check,
    ADD CHECK ((workspace_id IS NOT NULL AND account_kind IN ('customer_available','customer_reserved'))
            OR (workspace_id IS NULL AND account_kind IN ('payment_clearing','inference_revenue')));
INSERT INTO relay_hosted.ledger_accounts(account_kind) VALUES('inference_revenue');
INSERT INTO relay_hosted.ledger_accounts(workspace_id,account_kind) SELECT id,'customer_reserved' FROM relay_hosted.workspaces;
CREATE OR REPLACE FUNCTION relay_hosted.init_personal_wallet()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    INSERT INTO relay_hosted.wallets(workspace_id) VALUES(NEW.id);
    INSERT INTO relay_hosted.ledger_accounts(workspace_id,account_kind)
        VALUES(NEW.id,'customer_available'),(NEW.id,'customer_reserved');
    RETURN NULL;
END $$;
ALTER TABLE relay_hosted.journal_transactions DROP CONSTRAINT journal_transactions_operation_kind_check,
    ALTER COLUMN payment_id DROP NOT NULL, ADD COLUMN request_id uuid,
    ADD FOREIGN KEY(workspace_id,request_id) REFERENCES relay_hosted.text_requests(workspace_id,id),
    ADD UNIQUE(request_id,operation_kind),
    ADD CHECK ((operation_kind='payment_credit' AND payment_id IS NOT NULL AND request_id IS NULL)
        OR (operation_kind IN ('request_hold','request_close') AND payment_id IS NULL AND request_id IS NOT NULL));
ALTER TABLE relay_hosted.journal_postings DROP CONSTRAINT journal_postings_sequence_check,
    ADD CHECK(sequence BETWEEN 1 AND 3);
DROP TRIGGER balanced_payment_credit ON relay_hosted.journal_transactions;
CREATE CONSTRAINT TRIGGER balanced_payment_credit AFTER INSERT ON relay_hosted.journal_transactions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.operation_kind='payment_credit')
    EXECUTE FUNCTION relay_hosted.check_credit_postings();

-- Calculate postings from the frozen hold and the closed charge, never from caller-selected accounts.
CREATE FUNCTION relay_hosted.request_lines(rid uuid, kind text)
RETURNS TABLE(seq smallint, account uuid, amount bigint) LANGUAGE sql STABLE SET search_path=pg_catalog AS $$
    SELECT e.seq,a.id,e.amount FROM relay_hosted.text_requests r
    CROSS JOIN LATERAL (VALUES
        (1::smallint,r.workspace_id,'customer_available', CASE kind WHEN 'request_hold' THEN -r.held ELSE r.held-r.charged END),
        (2::smallint,r.workspace_id,'customer_reserved', CASE kind WHEN 'request_hold' THEN r.held ELSE -r.held END),
        (3::smallint,NULL::uuid,'inference_revenue',CASE kind WHEN 'request_close' THEN r.charged END)
    ) e(seq,workspace,kind,amount) JOIN relay_hosted.ledger_accounts a ON a.account_kind=e.kind AND a.workspace_id IS NOT DISTINCT FROM e.workspace
    WHERE r.id=rid AND e.amount<>0
$$;
CREATE FUNCTION relay_hosted.record_request_journal(r relay_hosted.text_requests, kind text)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE receipt uuid;
BEGIN
    INSERT INTO relay_hosted.journal_transactions(workspace_id,operation_kind,request_id)
        VALUES(r.workspace_id,kind,r.id) RETURNING id INTO receipt;
    INSERT INTO relay_hosted.journal_postings(transaction_id,sequence,account_id,amount_micro_usd)
        SELECT receipt,seq,account,amount FROM relay_hosted.request_lines(r.id,kind);
END $$;
CREATE FUNCTION relay_hosted.check_request_postings()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE j relay_hosted.journal_transactions; jid uuid;
BEGIN
    IF TG_TABLE_NAME='journal_transactions' THEN jid:=NEW.id; ELSE jid:=NEW.transaction_id; END IF;
    SELECT * INTO STRICT j FROM relay_hosted.journal_transactions WHERE id=jid;
    IF j.operation_kind='payment_credit' THEN
        IF EXISTS (SELECT FROM relay_hosted.journal_postings WHERE transaction_id=jid AND sequence>2) THEN
            RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='extra payment posting'; END IF;
    ELSIF EXISTS (SELECT FROM relay_hosted.request_lines(j.request_id,j.operation_kind) e
        FULL JOIN (SELECT * FROM relay_hosted.journal_postings WHERE transaction_id=jid) a
        ON (e.seq,e.account,e.amount)=(a.sequence,a.account_id,a.amount_micro_usd)
        WHERE e.seq IS NULL OR a.sequence IS NULL) THEN
        RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='invalid request postings';
    END IF; RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER correct_request_transfer AFTER INSERT ON relay_hosted.journal_transactions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.check_request_postings();
CREATE CONSTRAINT TRIGGER correct_posting_line AFTER INSERT ON relay_hosted.journal_postings
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.check_request_postings();
REVOKE ALL ON FUNCTION relay_hosted.request_lines(uuid,text),
    relay_hosted.record_request_journal(relay_hosted.text_requests,text),relay_hosted.check_request_postings() FROM PUBLIC;
