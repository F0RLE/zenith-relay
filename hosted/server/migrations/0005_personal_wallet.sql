CREATE TABLE relay_hosted.workspaces (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_user_id uuid NOT NULL UNIQUE REFERENCES relay_hosted.users,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- One USD wallet per personal workspace. No opening balance or journal entry.
CREATE TABLE relay_hosted.wallets (
    workspace_id uuid PRIMARY KEY REFERENCES relay_hosted.workspaces,
    currency text NOT NULL DEFAULT 'USD' CHECK (currency = 'USD'),
    available_micro_usd bigint NOT NULL DEFAULT 0 CHECK (available_micro_usd >= 0),
    reserved_micro_usd bigint NOT NULL DEFAULT 0 CHECK (reserved_micro_usd >= 0),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    CHECK (available_micro_usd <= 9223372036854775807 - reserved_micro_usd)
);

CREATE TABLE relay_hosted.ledger_accounts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid REFERENCES relay_hosted.wallets,
    account_kind text NOT NULL,
    UNIQUE (workspace_id, account_kind),
    CHECK ((workspace_id IS NOT NULL AND account_kind = 'customer_available')
        OR (workspace_id IS NULL AND account_kind = 'payment_clearing'))
);
CREATE UNIQUE INDEX installation_ledger_account ON relay_hosted.ledger_accounts (account_kind)
    WHERE workspace_id IS NULL;

INSERT INTO relay_hosted.ledger_accounts (account_kind) VALUES ('payment_clearing');

CREATE FUNCTION relay_hosted.init_personal_wallet()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO relay_hosted.wallets (workspace_id) VALUES (NEW.id);
    INSERT INTO relay_hosted.ledger_accounts (workspace_id, account_kind)
        VALUES (NEW.id, 'customer_available');
    RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.init_personal_wallet() FROM PUBLIC;
CREATE TRIGGER personal_wallet AFTER INSERT ON relay_hosted.workspaces
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.init_personal_wallet();
