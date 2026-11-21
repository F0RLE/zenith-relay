ALTER TABLE relay_hosted.checkout_builder_operations
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id),
    ADD CHECK (confirmed_by_passkey_id IS NULL OR action IN ('publish','disable'));
ALTER TABLE relay_hosted.payment_refund_reviews
    ADD COLUMN confirmed_by_passkey_id uuid REFERENCES relay_hosted.admin_credentials(passkey_id);

DROP FUNCTION relay_hosted.change_checkout_builder(text,bytea,uuid,bigint,text,jsonb,uuid);
DROP FUNCTION relay_hosted.review_payment_refund(text,bytea,uuid,uuid,text,text);

CREATE FUNCTION relay_hosted.lock_owner_payment_session(origin text,tag bytea)
RETURNS relay_hosted.admin_sessions LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE actor uuid; sess relay_hosted.admin_sessions;
BEGIN
    -- Fence recovery/revocation before taking the session and business locks.
    -- Do not share-lock the credential here: the assertion may advance it.
    SELECT u.id INTO actor FROM relay_hosted.installations i
        JOIN relay_hosted.service_control s ON s.installation_id=i.id
        JOIN relay_hosted.users u ON u.id=i.owner_user_id
        JOIN relay_hosted.admin_assignments a ON a.user_id=u.id
        WHERE i.singleton AND i.public_origin=origin AND i.setup_state='claimed' AND s.restore_operation IS NULL
            AND a.role='owner' AND a.revoked_at IS NULL AND u.disabled_at IS NULL
        FOR SHARE OF i,s,u,a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT t.* INTO sess FROM relay_hosted.admin_sessions t WHERE t.verifier=tag AND t.user_id=actor
        AND (relay_hosted.owner_passkey_session(origin,tag)).user_id=t.user_id FOR SHARE;
    RETURN sess;
END $$;

CREATE FUNCTION relay_hosted.confirm_owner_payment(sess relay_hosted.admin_sessions, credential bytea,
    expected_revision bigint, counter bigint, backup boolean, eligible boolean, deadline_ms bigint,
    OUT outcome text, OUT confirmer uuid, OUT verified_at timestamptz)
LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE key_row relay_hosted.admin_credentials; stamp_ms bigint;
BEGIN
    SELECT k.* INTO key_row FROM relay_hosted.admin_credentials k
        WHERE k.user_id=sess.user_id AND k.credential_id=credential AND k.revoked_at IS NULL
            AND k.revision=expected_revision FOR NO KEY UPDATE;
    verified_at:=clock_timestamp();
    stamp_ms:=floor(extract(epoch FROM verified_at)*1000)::bigint;
    IF sess.expires_at<=verified_at OR sess.last_seen_at<=verified_at-interval '20 minutes' THEN
        outcome:='denied';
    ELSIF deadline_ms IS NULL OR deadline_ms<=stamp_ms OR deadline_ms>stamp_ms+180000 THEN
        outcome:='expired';
    ELSIF key_row.passkey_id IS NULL OR backup IS NULL OR eligible IS NULL
        OR counter IS NULL OR counter NOT BETWEEN 0 AND 4294967295
        OR NOT (counter>(key_row.passkey #>> '{cred,counter}')::bigint
            OR (counter=0 AND (key_row.passkey #>> '{cred,counter}')::bigint=0)) IS TRUE THEN
        outcome:='passkey_changed';
    ELSE
        UPDATE relay_hosted.admin_credentials SET revision=revision+1, passkey=
            jsonb_set(jsonb_set(jsonb_set(passkey,'{cred,counter}',to_jsonb(counter)),
                '{cred,backup_state}',to_jsonb(backup)),'{cred,backup_eligible}',to_jsonb(eligible))
            WHERE passkey_id=key_row.passkey_id RETURNING passkey_id INTO confirmer;
        outcome:='verified';
    END IF;
END $$;

CREATE FUNCTION relay_hosted.change_checkout_builder(origin text,tag bytea,op uuid,expected bigint,
    verb text,draft_input jsonb,target uuid,proof_credential bytea,proof_revision bigint,proof_counter bigint,
    proof_backup boolean,proof_eligible boolean,proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE b relay_hosted.checkout_builder; prior relay_hosted.checkout_builder_operations;
    ss relay_hosted.admin_sessions; stamp timestamptz; fingerprint bytea; retired relay_hosted.checkout_methods;
    proof_result text; confirmed uuid;
BEGIN
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    IF op IS NULL OR op='00000000-0000-0000-0000-000000000000' OR expected IS NULL
        OR expected NOT BETWEEN 1 AND 9223372036854775806
        OR verb IS NULL OR verb NOT IN ('save','check','publish','disable')
        OR (verb='save')<>(draft_input IS NOT NULL) OR (verb='disable')<>(target IS NOT NULL)
        THEN RETURN 'invalid'; END IF;
    IF NOT relay_hosted.local_checkout(origin) THEN RETURN 'unavailable'; END IF;
    SELECT * INTO STRICT b FROM relay_hosted.checkout_builder FOR UPDATE;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    fingerprint:=sha256(convert_to(jsonb_build_array(draft_input,target)::text,'UTF8'));
    SELECT * INTO prior FROM relay_hosted.checkout_builder_operations WHERE operation_id=op;
    IF FOUND THEN
        IF (ss.user_id,expected,verb,fingerprint)=(prior.actor_id,prior.expected_revision,prior.action,prior.digest)
            THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF b.revision<>expected THEN RETURN 'conflict'; END IF;
    IF verb='publish' AND (b.checked_until IS NULL OR b.checked_until<=stamp) THEN RETURN 'check_required'; END IF;
    IF verb='disable' THEN
        SELECT * INTO retired FROM relay_hosted.checkout_methods WHERE id=target AND disabled_at IS NULL FOR NO KEY UPDATE;
        IF NOT FOUND THEN RETURN 'conflict'; END IF;
    END IF;
    IF verb IN ('publish','disable') THEN
        IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
        SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
            ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
        IF proof_result<>'verified' THEN RETURN proof_result; END IF;
        IF verb='publish' AND b.checked_until<=stamp THEN RETURN 'check_required'; END IF;
    END IF;

    IF verb='save' THEN
        IF NOT relay_hosted.valid_checkout_draft(draft_input) THEN RETURN 'invalid'; END IF;
        UPDATE relay_hosted.checkout_builder SET draft=draft_input,revision=revision+1,checked_until=NULL;
    ELSIF verb='check' THEN
        IF NOT relay_hosted.valid_checkout_draft(b.draft) THEN RETURN 'invalid'; END IF;
        UPDATE relay_hosted.checkout_builder SET checked_until=stamp+interval '10 minutes';
    ELSIF verb='publish' THEN
        PERFORM relay_hosted.publish_checkout(b.draft,stamp);
        UPDATE relay_hosted.checkout_builder SET revision=revision+1,checked_until=NULL;
    ELSE
        UPDATE relay_hosted.checkout_methods SET disabled_at=stamp WHERE id=target;
        UPDATE relay_hosted.checkout_builder SET revision=revision+1,checked_until=NULL,
            draft=jsonb_set(b.draft,'{methods}',coalesce((SELECT jsonb_agg(s.item ORDER BY s.n)
                FROM jsonb_array_elements(b.draft->'methods') WITH ORDINALITY s(item,n)
                WHERE (s.item->>'binding_id',(s.item->>'code') COLLATE "C")
                    <>(retired.merchant_binding_id::text,retired.method_code)),'[]'));
    END IF;
    INSERT INTO relay_hosted.checkout_builder_operations(operation_id,actor_id,expected_revision,action,digest,
        target_method,publication,applied_at,confirmed_by_passkey_id)
        VALUES (op,ss.user_id,expected,verb,fingerprint,target,CASE WHEN verb='publish' THEN b.draft END,stamp,confirmed);
    RETURN 'applied';
END $$;

CREATE FUNCTION relay_hosted.review_payment_refund(origin text,tag bytea,target uuid,op uuid,expected text,verb text,
    proof_credential bytea,proof_revision bigint,proof_counter bigint,proof_backup boolean,proof_eligible boolean,
    proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; r relay_hosted.payment_refunds;
    i relay_hosted.payment_intents; w relay_hosted.wallets; stamp timestamptz; proof_result text; confirmed uuid;
BEGIN
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    IF target IS NULL OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS DISTINCT FROM 'requested' OR verb IS NULL OR verb NOT IN ('approve','decline')
        THEN RETURN 'invalid'; END IF;
    IF NOT relay_hosted.local_checkout(origin) THEN RETURN 'unavailable'; END IF;
    -- Match inference and reconciliation: take the wallet before the refund.
    SELECT * INTO w FROM relay_hosted.wallets x
        WHERE x.workspace_id=(SELECT workspace_id FROM relay_hosted.payment_refunds WHERE id=target) FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT r FROM relay_hosted.payment_refunds WHERE id=target FOR UPDATE;
    stamp:=clock_timestamp();
    IF ss.expires_at<=stamp OR ss.last_seen_at<=stamp-interval '20 minutes' THEN RETURN 'denied'; END IF;
    IF EXISTS (SELECT FROM relay_hosted.payment_refund_reviews x WHERE x.operation_id=op) THEN
        IF EXISTS (SELECT FROM relay_hosted.payment_refund_reviews x WHERE x.operation_id=op
            AND (x.refund_id,x.actor_id,x.action)=(target,ss.user_id,verb)) THEN RETURN 'applied'; END IF;
        RETURN 'conflict';
    END IF;
    IF r.state<>expected THEN RETURN 'conflict'; END IF;
    IF verb='approve' THEN
        SELECT * INTO STRICT i FROM relay_hosted.payment_intents WHERE id=r.payment_id;
        IF r.amount_micro_usd>relay_hosted.refundable_credit(i) THEN RETURN 'remainder'; END IF;
        IF r.amount_micro_usd>w.available_micro_usd THEN RETURN 'balance'; END IF;
    END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    SELECT outcome,confirmer,verified_at INTO proof_result,confirmed,stamp FROM relay_hosted.confirm_owner_payment(
        ss,proof_credential,proof_revision,proof_counter,proof_backup,proof_eligible,proof_deadline_ms);
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.payment_refund_reviews(refund_id,operation_id,actor_id,action,reason_code,reviewed_at,
        confirmed_by_passkey_id)
        VALUES(target,op,ss.user_id,verb,CASE verb WHEN 'approve' THEN 'customer_requested' ELSE 'owner_declined' END,
            stamp,confirmed)
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

REVOKE ALL ON FUNCTION relay_hosted.lock_owner_payment_session(text,bytea),
    relay_hosted.confirm_owner_payment(relay_hosted.admin_sessions,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.change_checkout_builder(text,bytea,uuid,bigint,text,jsonb,uuid,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.review_payment_refund(text,bytea,uuid,uuid,text,text,bytea,bigint,bigint,boolean,boolean,bigint)
    FROM PUBLIC;
