CREATE DOMAIN relay_hosted.payment_reference AS text COLLATE "C"
    CHECK (octet_length(VALUE) BETWEEN 1 AND 128 AND VALUE !~ '[^!-~]');

CREATE TABLE relay_hosted.merchant_bindings (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    provider_id relay_hosted.payment_reference NOT NULL,
    environment text NOT NULL CHECK (environment IN ('sandbox', 'live')),
    merchant_identity relay_hosted.payment_reference NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (provider_id, environment, merchant_identity)
);

CREATE TABLE relay_hosted.payment_intents (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES relay_hosted.wallets,
    operation_id uuid NOT NULL,
    quote_id uuid NOT NULL,
    merchant_binding_id uuid NOT NULL REFERENCES relay_hosted.merchant_bindings,
    merchant_reference relay_hosted.payment_reference,
    quoted_currency text COLLATE "C" NOT NULL CHECK (quoted_currency ~ '^[A-Z]{3}$'),
    quoted_amount_minor bigint NOT NULL CHECK (quoted_amount_minor > 0),
    credit_micro_usd bigint NOT NULL CHECK (credit_micro_usd > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, operation_id),
    UNIQUE (workspace_id, quote_id),
    UNIQUE (merchant_binding_id, merchant_reference),
    UNIQUE (workspace_id, id),
    UNIQUE (merchant_binding_id, id)
);

CREATE TABLE relay_hosted.payment_events (
    merchant_binding_id uuid NOT NULL,
    provider_event_id relay_hosted.payment_reference NOT NULL,
    payment_id uuid NOT NULL,
    verified_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (merchant_binding_id, provider_event_id),
    FOREIGN KEY (merchant_binding_id, payment_id)
        REFERENCES relay_hosted.payment_intents (merchant_binding_id, id)
);

CREATE TABLE relay_hosted.journal_transactions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL,
    operation_kind text NOT NULL CHECK (operation_kind = 'payment_credit'),
    payment_id uuid NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (workspace_id, payment_id) REFERENCES relay_hosted.payment_intents (workspace_id, id)
);
CREATE INDEX journal_wallet_history ON relay_hosted.journal_transactions (workspace_id, created_at DESC, id DESC);

CREATE TABLE relay_hosted.journal_postings (
    transaction_id uuid NOT NULL REFERENCES relay_hosted.journal_transactions,
    sequence smallint NOT NULL CHECK (sequence IN (1, 2)),
    account_id uuid NOT NULL REFERENCES relay_hosted.ledger_accounts,
    amount_micro_usd bigint NOT NULL CHECK (amount_micro_usd <> 0),
    PRIMARY KEY (transaction_id, sequence),
    UNIQUE (account_id, transaction_id)
);

CREATE FUNCTION relay_hosted.reject_financial_rewrite()
RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'immutable financial record';
END $$;
REVOKE ALL ON FUNCTION relay_hosted.reject_financial_rewrite() FROM PUBLIC;
CREATE TRIGGER stable_merchant BEFORE UPDATE OF provider_id, environment, merchant_identity
    ON relay_hosted.merchant_bindings FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER fixed_invoice_terms BEFORE UPDATE OF workspace_id, operation_id, quote_id, merchant_binding_id,
    quoted_currency, quoted_amount_minor, credit_micro_usd ON relay_hosted.payment_intents
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER fixed_invoice_reference BEFORE UPDATE OF merchant_reference ON relay_hosted.payment_intents
    FOR EACH ROW WHEN (OLD.merchant_reference IS NOT NULL AND OLD.merchant_reference IS DISTINCT FROM NEW.merchant_reference)
    EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER stable_ledger_account BEFORE UPDATE OF workspace_id, account_kind ON relay_hosted.ledger_accounts
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER immutable_payment_event BEFORE UPDATE OR DELETE ON relay_hosted.payment_events
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER immutable_journal_transaction BEFORE UPDATE OR DELETE ON relay_hosted.journal_transactions
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE TRIGGER immutable_journal_posting BEFORE UPDATE OR DELETE ON relay_hosted.journal_postings
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

-- Neither an empty receipt nor a balanced transfer to the wrong wallet may commit.
CREATE FUNCTION relay_hosted.check_credit_postings()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    IF NOT EXISTS (
        SELECT FROM relay_hosted.payment_intents i
        JOIN relay_hosted.journal_postings p ON p.transaction_id = NEW.id AND p.sequence = 1
        JOIN relay_hosted.ledger_accounts a ON a.id = p.account_id
        JOIN relay_hosted.journal_postings q ON q.transaction_id = NEW.id AND q.sequence = 2
        JOIN relay_hosted.ledger_accounts b ON b.id = q.account_id
        WHERE i.id = NEW.payment_id AND i.workspace_id = NEW.workspace_id
            AND a.workspace_id = i.workspace_id AND a.account_kind = 'customer_available'
            AND b.workspace_id IS NULL AND b.account_kind = 'payment_clearing'
            AND p.amount_micro_usd = i.credit_micro_usd AND q.amount_micro_usd = -i.credit_micro_usd
    ) THEN
        RAISE EXCEPTION USING ERRCODE = '23514', MESSAGE = 'invalid payment credit postings';
    END IF;
    RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.check_credit_postings() FROM PUBLIC;
CREATE CONSTRAINT TRIGGER balanced_payment_credit AFTER INSERT ON relay_hosted.journal_transactions
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION relay_hosted.check_credit_postings();

CREATE FUNCTION relay_hosted.post_verified_payment(binding uuid, event_ref text, invoice_ref text,
    paid_currency text, paid_minor bigint)
RETURNS uuid LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE invoice relay_hosted.payment_intents; receipt uuid;
BEGIN
    SELECT * INTO invoice FROM relay_hosted.payment_intents i
        WHERE i.merchant_binding_id = binding AND i.merchant_reference = invoice_ref COLLATE "C"
            AND i.quoted_currency = paid_currency COLLATE "C" AND i.quoted_amount_minor = paid_minor;
    IF NOT FOUND THEN RETURN NULL; END IF;

    INSERT INTO relay_hosted.payment_events (merchant_binding_id, provider_event_id, payment_id)
        VALUES (binding, event_ref, invoice.id) ON CONFLICT DO NOTHING;
    IF NOT FOUND AND NOT EXISTS (SELECT FROM relay_hosted.payment_events
        WHERE merchant_binding_id = binding AND provider_event_id = event_ref COLLATE "C" AND payment_id = invoice.id)
    THEN RAISE EXCEPTION USING ERRCODE = 'RH001', MESSAGE = 'payment event conflict'; END IF;

    PERFORM FROM relay_hosted.wallets WHERE workspace_id = invoice.workspace_id FOR NO KEY UPDATE;
    SELECT id INTO receipt FROM relay_hosted.journal_transactions WHERE payment_id = invoice.id;
    IF FOUND THEN RETURN receipt; END IF;

    UPDATE relay_hosted.wallets SET available_micro_usd = available_micro_usd + invoice.credit_micro_usd,
        revision = revision + 1 WHERE workspace_id = invoice.workspace_id
        AND invoice.credit_micro_usd <= 9223372036854775807 - available_micro_usd - reserved_micro_usd;
    IF NOT FOUND THEN RAISE EXCEPTION USING ERRCODE = 'RH002', MESSAGE = 'wallet capacity exceeded'; END IF;
    INSERT INTO relay_hosted.journal_transactions (workspace_id, operation_kind, payment_id)
        VALUES (invoice.workspace_id, 'payment_credit', invoice.id) RETURNING id INTO receipt;
    INSERT INTO relay_hosted.journal_postings (transaction_id, sequence, account_id, amount_micro_usd)
        SELECT receipt, 1, id, invoice.credit_micro_usd FROM relay_hosted.ledger_accounts
            WHERE workspace_id = invoice.workspace_id AND account_kind = 'customer_available'
        UNION ALL
        SELECT receipt, 2, id, -invoice.credit_micro_usd FROM relay_hosted.ledger_accounts
            WHERE workspace_id IS NULL AND account_kind = 'payment_clearing';
    RETURN receipt;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.post_verified_payment(uuid,text,text,text,bigint) FROM PUBLIC;
