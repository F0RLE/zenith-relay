ALTER TABLE relay_hosted.service_control ADD COLUMN restore_operation uuid,
    ADD CONSTRAINT restore_stays_in_maintenance CHECK (
        restore_operation IS NULL OR (mode='maintenance' AND restore_operation<>'00000000-0000-0000-0000-000000000000'));

CREATE OR REPLACE VIEW relay_hosted.closed_installation AS
    SELECT i.public_origin, s.restore_operation IS NOT NULL AS restore_pending
    FROM relay_hosted.installations i
    JOIN relay_hosted.service_control s ON s.installation_id=i.id
    WHERE i.singleton AND s.mode IN ('setup','qualification','maintenance');

-- Ordinary maintenance can still reconcile accepted payments. An isolated
-- restore cannot trust a missing credit or a restored, apparently unsent claim.
CREATE FUNCTION relay_hosted.reject_restored_payment_write()
RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
    PERFORM FROM relay_hosted.service_control WHERE restore_operation IS NULL FOR SHARE;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE='RH003', MESSAGE='restore requires payment reconciliation';
    END IF;
    RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.reject_restored_payment_write() FROM PUBLIC;

CREATE TRIGGER fence_restored_payment_event BEFORE INSERT ON relay_hosted.payment_events
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_restored_payment_write();
CREATE TRIGGER fence_restored_invoice_claim BEFORE UPDATE OF checkout_claimed_at ON relay_hosted.payment_intents
    FOR EACH ROW WHEN (OLD.checkout_claimed_at IS NULL AND NEW.checkout_claimed_at IS NOT NULL)
    EXECUTE FUNCTION relay_hosted.reject_restored_payment_write();
CREATE TRIGGER fence_restored_refund_claim BEFORE UPDATE OF claimed_at ON relay_hosted.payment_refunds
    FOR EACH ROW WHEN (OLD.claimed_at IS NULL AND NEW.claimed_at IS NOT NULL)
    EXECUTE FUNCTION relay_hosted.reject_restored_payment_write();
