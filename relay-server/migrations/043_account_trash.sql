CREATE TABLE account_trash (
    id TEXT PRIMARY KEY NOT NULL,
    data_json TEXT NOT NULL,
    secret_ref TEXT NOT NULL,
    trashed_at_ms INTEGER NOT NULL CHECK (trashed_at_ms >= 0)
);

-- Imports must not overwrite credentials retained by a trashed registration.
CREATE TRIGGER accounts_reject_trashed_id
BEFORE INSERT ON accounts
WHEN EXISTS (SELECT 1 FROM account_trash WHERE id = NEW.id OR secret_ref = NEW.secret_ref)
BEGIN
    SELECT RAISE(ABORT, 'account is in trash; restore it before importing');
END;
