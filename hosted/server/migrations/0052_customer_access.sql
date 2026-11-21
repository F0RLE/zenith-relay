ALTER TABLE relay_hosted.users ADD COLUMN closed_at timestamptz,
    ADD CONSTRAINT closed_customer_stays_disabled CHECK (
        closed_at IS NULL OR (disabled_at IS NOT NULL AND disabled_at=closed_at));

CREATE TABLE relay_hosted.customer_access_changes (
    operation_id uuid PRIMARY KEY CHECK (operation_id<>'00000000-0000-0000-0000-000000000000'),
    workspace_id uuid NOT NULL,
    user_id uuid NOT NULL,
    expected_revision bigint NOT NULL CHECK (expected_revision BETWEEN 1 AND 9223372036854775806),
    previous_state text NOT NULL CHECK (previous_state IN ('active','suspended')),
    action text NOT NULL,
    reason_code text NOT NULL,
    actor_id uuid NOT NULL,
    confirmed_by uuid NOT NULL,
    applied_at timestamptz NOT NULL CHECK (isfinite(applied_at)),
    UNIQUE (workspace_id,expected_revision),
    FOREIGN KEY (workspace_id,user_id) REFERENCES relay_hosted.workspaces(id,owner_user_id),
    CHECK ((action='suspend' AND previous_state='active' AND reason_code IN ('security_review','policy_review'))
        OR (action='reinstate' AND previous_state='suspended' AND reason_code='review_complete')
        OR (action='close' AND reason_code='verified_customer_request'))
);
CREATE TRIGGER immutable_customer_access_change BEFORE UPDATE OR DELETE ON relay_hosted.customer_access_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.customer_access_state(u relay_hosted.users)
RETURNS text LANGUAGE sql IMMUTABLE STRICT SET search_path=pg_catalog AS $$
    SELECT CASE WHEN (u).closed_at IS NOT NULL THEN 'closed'
        WHEN (u).disabled_at IS NOT NULL THEN 'suspended' ELSE 'active' END
$$;

-- Replay and live confirmation use the same transition. The epoch permanently
-- invalidates old sessions/keys, even when a suspension is lifted.
CREATE FUNCTION relay_hosted.apply_customer_access_change()
RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
    UPDATE relay_hosted.users u SET authorization_epoch=u.authorization_epoch+1,
        disabled_at=CASE WHEN NEW.action<>'reinstate' THEN NEW.applied_at END,
        closed_at=CASE WHEN NEW.action='close' THEN NEW.applied_at END
    WHERE u.id=NEW.user_id AND u.authorization_epoch=NEW.expected_revision
        AND relay_hosted.customer_access_state(u)=NEW.previous_state
        AND NOT EXISTS(SELECT FROM relay_hosted.admin_assignments a WHERE a.user_id=u.id);
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE='RH004', MESSAGE='customer access transition does not match';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER apply_customer_access AFTER INSERT ON relay_hosted.customer_access_changes
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.apply_customer_access_change();

CREATE FUNCTION relay_hosted.owner_customer_case(origin text, tag bytea, target uuid)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE result jsonb;
BEGIN
    IF (relay_hosted.owner_passkey_session(origin,tag)).verifier IS NULL THEN RETURN NULL; END IF;
    SELECT jsonb_build_object('workspace_id',w.id,'state',relay_hosted.customer_access_state(u),
        'revision',u.authorization_epoch::text, 'last_change',(
            SELECT jsonb_build_object('operation_id',c.operation_id,'action',c.action,'reason_code',c.reason_code,
                'applied_at_ms',floor(extract(epoch FROM c.applied_at)*1000)::bigint)
            FROM relay_hosted.customer_access_changes c WHERE c.workspace_id=w.id
            ORDER BY c.expected_revision DESC LIMIT 1))
    INTO result FROM relay_hosted.workspaces w JOIN relay_hosted.users u ON u.id=w.owner_user_id
    WHERE w.id=target AND NOT EXISTS(SELECT FROM relay_hosted.admin_assignments a WHERE a.user_id=u.id);
    RETURN coalesce(result,'{"error":"not_found"}'::jsonb);
END $$;

CREATE FUNCTION relay_hosted.change_customer_access(origin text, tag bytea, target uuid, op uuid,
    expected bigint, verb text, reason text, proof_credential bytea, proof_revision bigint,
    proof_counter bigint, proof_backup boolean, proof_eligible boolean, proof_deadline_ms bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.admin_sessions; u relay_hosted.users; prior relay_hosted.customer_access_changes; state text;
    proof_result text; confirmer uuid; stamp timestamptz;
BEGIN
    ss:=relay_hosted.lock_owner_payment_session(origin,tag);
    IF ss.verifier IS NULL THEN RETURN 'denied'; END IF;
    IF target IS NULL OR op IS NULL OR op='00000000-0000-0000-0000-000000000000'
        OR expected IS NULL OR expected NOT BETWEEN 1 AND 9223372036854775806
        OR NOT ((verb='suspend' AND reason IN ('security_review','policy_review'))
            OR (verb='reinstate' AND reason='review_complete')
            OR (verb='close' AND reason='verified_customer_request')) IS TRUE THEN RETURN 'invalid'; END IF;

    -- Customer authorization readers take SHARE here before the wallet.
    -- Do not lock or edit the wallet, pending operations, sessions or keys.
    SELECT person.* INTO u FROM relay_hosted.workspaces w JOIN relay_hosted.users person ON person.id=w.owner_user_id
    WHERE w.id=target AND NOT EXISTS(SELECT FROM relay_hosted.admin_assignments a WHERE a.user_id=person.id)
    FOR NO KEY UPDATE OF person;
    IF NOT FOUND THEN RETURN 'not_found'; END IF;
    IF ss.expires_at<=clock_timestamp() OR ss.last_seen_at<=clock_timestamp()-interval '20 minutes'
        THEN RETURN 'denied'; END IF;
    SELECT * INTO prior FROM relay_hosted.customer_access_changes c WHERE c.operation_id=op;
    IF FOUND THEN
        RETURN CASE WHEN (prior.workspace_id,prior.actor_id,prior.expected_revision,prior.action,prior.reason_code)
            =(target,ss.user_id,expected,verb,reason) THEN 'applied' ELSE 'conflict' END;
    END IF;
    state:=relay_hosted.customer_access_state(u);
    IF u.authorization_epoch<>expected OR state='closed' OR (verb='suspend' AND state<>'active')
        OR (verb='reinstate' AND state<>'suspended') THEN RETURN 'conflict'; END IF;
    IF proof_credential IS NULL THEN RETURN 'fresh_required'; END IF;
    SELECT p.outcome,p.confirmer,p.verified_at INTO proof_result,confirmer,stamp
        FROM relay_hosted.confirm_owner_payment(ss,proof_credential,proof_revision,
            proof_counter,proof_backup,proof_eligible,proof_deadline_ms) p;
    IF proof_result<>'verified' THEN RETURN proof_result; END IF;
    INSERT INTO relay_hosted.customer_access_changes(operation_id,workspace_id,user_id,expected_revision,
        previous_state,action,reason_code,actor_id,confirmed_by,applied_at)
        VALUES(op,target,u.id,expected,state,verb,reason,ss.user_id,confirmer,stamp);
    RETURN 'applied';
EXCEPTION WHEN unique_violation THEN
    -- Racing use of one operation against another workspace rolls back proof too.
    RETURN 'conflict';
END $$;

-- Pin the scope and its membership before taking wallet or request locks.
-- Login locks the qualification subject before the user and external identity.
CREATE FUNCTION relay_hosted.lock_text_customer(origin text,kid uuid,proof bytea)
RETURNS uuid LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE space uuid; scope uuid; subject bytea; provider uuid;
BEGIN
    space:=relay_hosted.qualified_key_workspace(origin,kid,proof);
    IF space IS NULL THEN RETURN NULL; END IF;
    SELECT s.qualification_scope_ref INTO scope
        FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON i.id=s.installation_id
        WHERE i.public_origin=origin AND s.mode='qualification' FOR SHARE OF i,s;
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT a.subject_digest,q.provider_id INTO subject,provider
        FROM relay_hosted.qualification_subjects a JOIN relay_hosted.configuration_revisions q ON q.id=a.scope_id
        WHERE a.scope_id=scope AND a.workspace_id=space FOR SHARE OF a,q;
    IF NOT FOUND THEN RETURN NULL; END IF;
    PERFORM FROM relay_hosted.users u JOIN relay_hosted.workspaces w ON w.owner_user_id=u.id
        JOIN relay_hosted.external_identities e ON e.user_id=u.id
        WHERE w.id=space AND e.provider_id=provider AND e.subject_digest=subject
            AND u.disabled_at IS NULL AND e.revoked_at IS NULL FOR SHARE OF u,w,e;
    IF NOT FOUND OR relay_hosted.qualified_key_workspace(origin,kid,proof) IS DISTINCT FROM space THEN RETURN NULL; END IF;
    RETURN space;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.lock_text_authority(origin text,oid uuid,kid uuid,proof bytea)
RETURNS uuid LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE space uuid; until_at timestamptz;
BEGIN
    space:=relay_hosted.lock_text_customer(origin,kid,proof);
    IF space IS NULL THEN RETURN NULL; END IF;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT least(k.expires_at,q.expires_at) INTO until_at
    FROM relay_hosted.customer_keys k JOIN relay_hosted.configuration_revisions q ON q.id=k.scope_id
    JOIN relay_hosted.text_offers o ON o.id=oid AND o.scope_id=q.id
    JOIN relay_hosted.text_model_rules r ON r.id=o.inventory_id AND o.revision>=r.first_live_revision
    WHERE k.id=kid AND k.workspace_id=space AND k.verifier=proof AND k.revoked_at IS NULL
        AND r.admitted AND r.enabled FOR SHARE OF k,r;
    IF NOT FOUND OR until_at<=clock_timestamp() THEN RETURN NULL; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.text_offers o CROSS JOIN LATERAL relay_hosted.text_offer_supply(o) p
        WHERE o.id=oid) THEN RETURN NULL; END IF;
    RETURN space;
END $$;

-- Ensure both direct and published reservation paths acquire the epoch lock
-- before the wallet. Keep the existing reservation and settlement contract.
CREATE OR REPLACE FUNCTION relay_hosted.reserve_text(origin text,oid uuid,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer,wants_stream boolean)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; r relay_hosted.text_requests; balance bigint; stamp timestamptz; sized integer; supply uuid[];
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid;
    space:=relay_hosted.lock_text_customer(origin,kid,proof);
    IF space IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT w.available_micro_usd INTO STRICT balance FROM relay_hosted.wallets w WHERE w.workspace_id=space FOR NO KEY UPDATE;
    IF relay_hosted.qualified_key_workspace(origin,kid,proof) IS DISTINCT FROM space THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    IF op IS NOT NULL THEN
        SELECT * INTO r FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.key_id=kid AND x.operation_tag=op;
        IF FOUND THEN
            IF r.input_tag IS DISTINCT FROM body_tag OR r.streaming IS DISTINCT FROM wants_stream THEN result:='conflict';
            ELSE rid:=r.id; result:=CASE WHEN r.closed_at IS NULL THEN 'in_progress' ELSE 'finished' END; END IF; space:=NULL;
            RETURN NEXT; RETURN;
        END IF;
    END IF;
    IF o.id IS NULL OR requested_model IS DISTINCT FROM o.model OR input_bytes IS NULL OR output_cap IS NULL
        OR wants_stream IS NULL OR input_bytes NOT BETWEEN 1 AND o.max_input_bytes OR output_cap NOT BETWEEN 1 AND o.max_output_tokens THEN
        result:='unoffered'; RETURN NEXT; RETURN; END IF;
    IF relay_hosted.lock_text_authority(origin,oid,kid,proof) IS DISTINCT FROM space THEN
        result:='unoffered'; RETURN NEXT; RETURN; END IF;
    SELECT array_agg(p.id ORDER BY p.id) INTO supply FROM relay_hosted.text_offer_supply(o) p
        WHERE NOT wants_stream OR EXISTS(SELECT FROM relay_hosted.text_stream_checks c WHERE c.inventory_id=p.id);
    IF supply IS NULL THEN result:='unoffered'; RETURN NEXT; RETURN; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.closed_at IS NULL)>=20 THEN
        result:='capacity'; RETURN NEXT; RETURN; END IF;
    SELECT s.admission_epoch INTO epoch FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id;
    sized:=input_bytes*o.tokens_per_byte+o.overhead_tokens;
    held:=relay_hosted.text_cost_bound(sized,output_cap,o.input_rate,o.cached_rate,o.output_rate);
    IF balance<held THEN result:='funds'; RETURN NEXT; RETURN; END IF;
    stamp:=clock_timestamp();
    INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,operation_tag,input_tag,send_tag,
        input_bound,output_bound,held,started_at,claim_by,close_by,supply_items,streaming)
    VALUES(space,kid,oid,epoch,op,body_tag,claim_tag,sized,output_cap,held,stamp,
        stamp+interval '30 seconds',stamp+o.close_after_s*interval '1 second',supply,wants_stream) RETURNING * INTO r;
    UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd-r.held,reserved_micro_usd=w.reserved_micro_usd+r.held,
        revision=w.revision+1 WHERE w.workspace_id=space;
    PERFORM relay_hosted.record_request_journal(r,'request_hold');
    result:='new'; rid:=r.id; attempt:=r.attempt_id; RETURN NEXT;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.reserve_published_text(origin text,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer,wants_stream boolean)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint,offer uuid,tariff bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE oid uuid; v bigint; workspace uuid;
BEGIN
    workspace:=relay_hosted.lock_text_customer(origin,kid,proof);
    IF workspace IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=workspace FOR NO KEY UPDATE;
    SELECT l.id,l.revision INTO oid,v FROM relay_hosted.active_text_listings l
        JOIN relay_hosted.text_model_rules r ON r.id=l.item AND r.last_offer=l.id AND r.enabled
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id AND q.workspace_id=workspace
        WHERE l.model=requested_model AND l.public_origin=origin FOR SHARE OF r;
    RETURN QUERY SELECT x.*,oid,v FROM relay_hosted.reserve_text(origin,oid,kid,proof,op,body_tag,claim_tag,
        requested_model,input_bytes,output_cap,wants_stream) x;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.key_summary(k relay_hosted.customer_keys, scope uuid)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('id',k.id,'operation_id',k.issue_operation,'name',k.label,
        'prefix','rlk_'||left(replace(k.id::text,'-',''),12),'replaces_key_id',k.replaces,
        'expires_at_s',floor(extract(epoch FROM k.expires_at))::bigint,
        'status',CASE WHEN k.revoked_at IS NOT NULL OR NOT EXISTS(
            SELECT FROM relay_hosted.workspaces w JOIN relay_hosted.users u ON u.id=w.owner_user_id
            WHERE w.id=k.workspace_id AND u.authorization_epoch=k.user_epoch) THEN 'revoked'
            WHEN k.scope_id<>scope OR k.expires_at<=statement_timestamp() THEN 'expired' ELSE 'active' END)
$$;

CREATE OR REPLACE FUNCTION relay_hosted.issue_customer_key(origin text, provider uuid, binding bytea, token bytea,
    op uuid, name text, original uuid, requested bigint, new_id uuid, new_verifier bytea)
RETURNS TABLE(outcome text, summary jsonb) LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE ss relay_hosted.customer_sessions; k relay_hosted.customer_keys; until_at timestamptz; stamp timestamptz;
BEGIN
    ss := relay_hosted.key_authority(origin,provider,binding,token,true);
    IF ss.verifier IS NULL THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT * INTO k FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.issue_operation=op;
    IF FOUND THEN
        IF k.label=name AND k.replaces IS NOT DISTINCT FROM original AND k.requested_expiry IS NOT DISTINCT FROM requested THEN
            outcome:='existing'; summary:=relay_hosted.key_summary(k,ss.scope_id);
        ELSE outcome:='conflict'; END IF;
        RETURN NEXT; RETURN;
    END IF;
    SELECT r.expires_at INTO STRICT until_at FROM relay_hosted.configuration_revisions r WHERE r.id=ss.scope_id;
    IF original IS NOT NULL THEN
        SELECT * INTO k FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.id=original;
        IF NOT FOUND THEN outcome:='missing'; RETURN NEXT; RETURN; END IF;
        IF k.revoked_at IS NOT NULL OR k.scope_id<>ss.scope_id OR k.expires_at<=clock_timestamp()
            OR (k.user_epoch,k.installation_epoch)<>(ss.user_epoch,ss.installation_epoch) THEN
            outcome:='conflict'; RETURN NEXT; RETURN;
        END IF;
        until_at:=least(until_at,k.expires_at);
    END IF;
    stamp:=clock_timestamp();
    IF requested IS NOT NULL THEN
        IF requested<=extract(epoch FROM stamp) OR requested>extract(epoch FROM until_at) THEN
            outcome:='expiry'; RETURN NEXT; RETURN;
        END IF;
        until_at:=to_timestamp(requested);
    END IF;
    IF until_at<=stamp THEN outcome:='denied'; RETURN NEXT; RETURN; END IF;
    IF (SELECT count(*) FROM relay_hosted.customer_keys x WHERE x.workspace_id=ss.workspace_id AND x.scope_id=ss.scope_id
        AND x.user_epoch=ss.user_epoch)>=32 THEN
        outcome:='limit'; RETURN NEXT; RETURN;
    END IF;
    INSERT INTO relay_hosted.customer_keys(id,workspace_id,scope_id,issue_operation,label,replaces,requested_expiry,verifier,
        user_epoch,installation_epoch,created_at,expires_at)
        VALUES(new_id,ss.workspace_id,ss.scope_id,op,name,original,requested,new_verifier,
            ss.user_epoch,ss.installation_epoch,stamp,until_at) RETURNING * INTO k;
    outcome:='issued'; summary:=relay_hosted.key_summary(k,ss.scope_id); RETURN NEXT;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.owner_refund_cases(origin text,tag bytea,target uuid,before_id uuid)
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
        FROM (SELECT r.id,r.requested_at,relay_hosted.refund_summary(r) ||
                jsonb_build_object('label',m.label,'workspace_id',i.workspace_id) item
            FROM relay_hosted.payment_refunds r JOIN relay_hosted.payment_intents i ON i.id=r.payment_id
            JOIN relay_hosted.checkout_methods m ON m.id=i.checkout_method_id
            WHERE (target IS NULL OR r.payment_id=target)
                AND (before_id IS NULL OR (r.requested_at,r.id)<(boundary,before_id))
            ORDER BY r.requested_at DESC,r.id DESC LIMIT 33) p);
END $$;

-- Migration owner only, on the isolated target. Evidence comes from the
-- authoritative installation outside the restored snapshot.
CREATE FUNCTION relay_hosted.replay_customer_access(origin text, installation uuid, recovery uuid, receipts jsonb)
RETURNS integer LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE incoming relay_hosted.customer_access_changes; prior relay_hosted.customer_access_changes; added integer:=0;
BEGIN
    PERFORM FROM relay_hosted.installations i JOIN relay_hosted.service_control s ON s.installation_id=i.id
        WHERE i.id=installation AND i.public_origin=origin AND s.restore_operation=recovery AND s.mode='maintenance'
        FOR UPDATE OF i,s;
    IF NOT FOUND THEN
        RAISE EXCEPTION USING ERRCODE='RH004', MESSAGE='customer access replay requires the matching restore fence';
    END IF;
    FOR incoming IN SELECT * FROM jsonb_populate_recordset(NULL::relay_hosted.customer_access_changes,receipts)
        ORDER BY workspace_id,expected_revision
    LOOP
        SELECT * INTO prior FROM relay_hosted.customer_access_changes WHERE operation_id=incoming.operation_id;
        IF FOUND THEN
            IF prior IS DISTINCT FROM incoming THEN
                RAISE EXCEPTION USING ERRCODE='RH004', MESSAGE='customer access receipt does not match';
            END IF;
        ELSE
            INSERT INTO relay_hosted.customer_access_changes SELECT incoming.*;
            added:=added+1;
        END IF;
    END LOOP;
    RETURN added;
END $$;

REVOKE ALL ON FUNCTION relay_hosted.customer_access_state(relay_hosted.users),relay_hosted.apply_customer_access_change(),
    relay_hosted.owner_customer_case(text,bytea,uuid),
    relay_hosted.change_customer_access(text,bytea,uuid,uuid,bigint,text,text,bytea,bigint,bigint,boolean,boolean,bigint),
    relay_hosted.lock_text_customer(text,uuid,bytea),relay_hosted.replay_customer_access(text,uuid,uuid,jsonb) FROM PUBLIC;
