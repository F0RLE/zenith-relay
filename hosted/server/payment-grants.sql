GRANT USAGE ON SCHEMA relay_hosted TO :"payment_role";
GRANT EXECUTE ON FUNCTION relay_hosted.post_verified_payment(uuid,text,text,text,bigint) TO :"payment_role";
REVOKE EXECUTE ON FUNCTION relay_hosted.record_checkout_invoice(uuid,uuid,text,text,bigint,bigint,boolean)
    FROM :"payment_role";
GRANT EXECUTE ON FUNCTION relay_hosted.claim_checkout_invoice(uuid),relay_hosted.checkout_invoice_for_lookup(uuid,uuid),
    relay_hosted.observe_checkout_invoice(uuid,uuid,text,text,bigint,bigint,boolean,boolean),
    relay_hosted.claim_checkout_resume(uuid),relay_hosted.finish_checkout_resume(uuid,uuid,uuid,text),
    relay_hosted.register_checkout_profile(text,uuid,text,jsonb) TO :"payment_role";
GRANT EXECUTE ON FUNCTION relay_hosted.claim_payment_refund(uuid),relay_hosted.pending_refund_lookups(uuid,uuid),
    relay_hosted.refund_for_lookup(uuid,uuid),relay_hosted.observe_payment_refund(uuid,uuid,text,text,bigint,text,boolean)
    TO :"payment_role";
