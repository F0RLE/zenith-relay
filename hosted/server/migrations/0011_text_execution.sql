CREATE FUNCTION relay_hosted.text_offer_config(oid uuid, origin text)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('source',o.source_id,'model',o.model,'endpoint',o.base_url,'revision',o.revision,
        'provider_revision',o.provider_revision,'input_bytes',o.max_input_bytes,'output_cap',o.max_output_tokens)
    FROM relay_hosted.text_offers o JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id
    JOIN relay_hosted.installations i ON i.id=s.installation_id
    JOIN relay_hosted.configuration_revisions r ON r.id=o.scope_id
    WHERE o.id=oid AND i.public_origin=origin AND s.mode='qualification' AND r.expires_at>statement_timestamp()
        AND NOT EXISTS(SELECT FROM relay_hosted.text_fences f WHERE f.offer_id=o.id)
$$;

-- Locks are held through reservation/claim only, never through HTTP. Recheck expiry
-- with the database wall clock after all admission locks (including the wallet).
CREATE FUNCTION relay_hosted.lock_text_authority(origin text, oid uuid, kid uuid, proof bytea)
RETURNS uuid LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE space uuid; until_at timestamptz;
BEGIN
    -- Pin the vault generation through the decision. This first route is explicitly
    -- direct; an assigned proxy cannot be dropped to make it eligible.
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT k.workspace_id,least(k.expires_at,r.expires_at) INTO space,until_at
        FROM relay_hosted.customer_keys k JOIN relay_hosted.text_offers o ON o.scope_id=k.scope_id
        JOIN relay_hosted.service_control s ON s.qualification_scope_ref=o.scope_id
        JOIN relay_hosted.installations i ON i.id=s.installation_id
        JOIN relay_hosted.configuration_revisions r ON r.id=k.scope_id
        JOIN relay_hosted.text_grants g ON g.offer_id=o.id AND g.workspace_id=k.workspace_id
        JOIN relay_hosted.workspaces w ON w.id=k.workspace_id
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=k.scope_id AND q.workspace_id=w.id
        JOIN relay_hosted.external_identities e ON e.provider_id=r.provider_id AND e.subject_digest=q.subject_digest
        JOIN relay_hosted.users u ON u.id=e.user_id AND u.id=w.owner_user_id
        JOIN relay_hosted.connection_secret_slots v ON v.installation_id=i.id AND v.purpose='provider'
            AND v.connection_id=o.source_id AND v.revision=o.provider_revision AND v.generation IS NOT NULL
        WHERE o.id=oid AND i.public_origin=origin AND s.mode='qualification' AND k.id=kid AND k.verifier=proof
            AND k.installation_epoch=i.authorization_epoch AND k.user_epoch=u.authorization_epoch AND k.revoked_at IS NULL
            AND e.revoked_at IS NULL AND u.disabled_at IS NULL AND k.expires_at>clock_timestamp() AND r.expires_at>clock_timestamp()
            AND NOT EXISTS(SELECT FROM relay_hosted.connection_secret_slots proxy WHERE proxy.installation_id=i.id
                AND proxy.connection_id=o.source_id AND proxy.purpose='proxy' AND proxy.generation IS NOT NULL)
        FOR SHARE OF i,s,k,r,g,q,w,e,u,v;
    IF until_at<=clock_timestamp() THEN RETURN NULL; END IF;
    RETURN space;
END $$;

CREATE FUNCTION relay_hosted.reserve_text(origin text,oid uuid,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; r relay_hosted.text_requests; balance bigint; stamp timestamptz; sized integer;
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid;
    space:=relay_hosted.qualified_key_workspace(origin,kid,proof);
    IF space IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    SELECT w.available_micro_usd INTO STRICT balance FROM relay_hosted.wallets w WHERE w.workspace_id=space FOR NO KEY UPDATE;
    IF relay_hosted.qualified_key_workspace(origin,kid,proof) IS DISTINCT FROM space THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    IF op IS NOT NULL THEN
        SELECT * INTO r FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.key_id=kid AND x.operation_tag=op;
        IF FOUND THEN
            IF r.input_tag IS DISTINCT FROM body_tag THEN result:='conflict';
            ELSE rid:=r.id; result:=CASE WHEN r.closed_at IS NULL THEN 'in_progress' ELSE 'finished' END; END IF; space:=NULL;
            RETURN NEXT; RETURN;
        END IF;
    END IF;
    IF o.id IS NULL OR requested_model IS DISTINCT FROM o.model OR input_bytes IS NULL OR output_cap IS NULL
        OR input_bytes NOT BETWEEN 1 AND o.max_input_bytes OR output_cap NOT BETWEEN 1 AND o.max_output_tokens THEN
        result:='unoffered'; RETURN NEXT; RETURN; END IF;
    -- Scope changes take effect at the actual permit as well. Locking the immutable
    -- offer against fencing prevents a concurrent bound breach admitting a new hold.
    PERFORM FROM relay_hosted.text_offers WHERE id=oid FOR SHARE;
    IF relay_hosted.lock_text_authority(origin,oid,kid,proof) IS DISTINCT FROM space
        OR EXISTS(SELECT FROM relay_hosted.text_fences WHERE offer_id=oid) THEN result:='unoffered'; RETURN NEXT; RETURN; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_requests x WHERE x.workspace_id=space AND x.closed_at IS NULL)>=20 THEN
        result:='capacity'; RETURN NEXT; RETURN; END IF;
    SELECT s.admission_epoch INTO epoch FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id;
    sized:=input_bytes*o.tokens_per_byte+o.overhead_tokens;
    held:=ceil(sized*greatest(o.input_rate,o.cached_rate)+output_cap*o.output_rate)::bigint;
    IF balance<held THEN result:='funds'; RETURN NEXT; RETURN; END IF;
    stamp:=clock_timestamp();
    INSERT INTO relay_hosted.text_requests(workspace_id,key_id,offer_id,authority_epoch,operation_tag,input_tag,send_tag,input_bound,output_bound,
        held,started_at,claim_by,close_by) VALUES(space,kid,oid,epoch,op,body_tag,claim_tag,sized,output_cap,held,stamp,
        stamp+interval '30 seconds',stamp+o.close_after_s*interval '1 second') RETURNING * INTO r;
    UPDATE relay_hosted.wallets w SET available_micro_usd=w.available_micro_usd-r.held,reserved_micro_usd=w.reserved_micro_usd+r.held,
        revision=w.revision+1 WHERE w.workspace_id=space;
    PERFORM relay_hosted.record_request_journal(r,'request_hold');
    result:='new'; rid:=r.id; attempt:=r.attempt_id; RETURN NEXT;
END $$;

CREATE FUNCTION relay_hosted.claim_text(origin text,oid uuid,rid uuid,aid uuid,space uuid,kid uuid,
    source uuid,model text,auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer)
RETURNS boolean LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE proof bytea; o relay_hosted.text_offers;
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid FOR NO KEY UPDATE;
    IF NOT FOUND OR (o.source_id,o.model,o.revision,o.max_input_bytes)<>(source,model,tariff_rev,input_cap)
        OR EXISTS(SELECT FROM relay_hosted.text_fences WHERE offer_id=oid) THEN RETURN false; END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||source::text,0));
    SELECT verifier INTO proof FROM relay_hosted.customer_keys k WHERE k.id=kid AND k.workspace_id=space;
    IF proof IS NULL OR relay_hosted.lock_text_authority(origin,oid,kid,proof) IS DISTINCT FROM space THEN RETURN false; END IF;
    IF NOT EXISTS(SELECT FROM relay_hosted.service_control s WHERE s.qualification_scope_ref=o.scope_id AND s.admission_epoch=auth_rev)
        OR EXISTS(SELECT FROM relay_hosted.text_requests x JOIN relay_hosted.text_offers p ON p.id=x.offer_id
            WHERE p.source_id=source AND x.execution IN ('dispatched','uncertain') AND x.closed_at IS NULL)
        THEN RETURN false; END IF;
    UPDATE relay_hosted.text_requests r SET execution='dispatched',claimed_at=clock_timestamp()
        WHERE (r.id,r.attempt_id,r.workspace_id,r.key_id,r.offer_id,r.held,r.send_tag,r.output_bound)
            =(rid,aid,space,kid,oid,bound,tag,output_cap) AND r.authority_epoch=auth_rev
            AND r.execution='reserved' AND r.claim_by>clock_timestamp();
    RETURN FOUND;
END $$;

-- The caller holds the request lock. No caller chooses the debit or its destination.
CREATE FUNCTION relay_hosted.close_text(r relay_hosted.text_requests, next_execution text, evidence text)
RETURNS void LANGUAGE plpgsql SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; value numeric; debit bigint; held_amount bigint:=r.held;
BEGIN
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=r.offer_id;
    value:=CASE WHEN evidence IN ('provider_final','provider_partial') THEN ceil(
        CASE WHEN r.input_tokens IS NULL THEN 0 WHEN r.cached_tokens IS NULL THEN
            CASE WHEN o.input_rate=o.cached_rate THEN r.input_tokens*o.input_rate ELSE 0 END
        ELSE (r.input_tokens-r.cached_tokens)*o.input_rate+r.cached_tokens*o.cached_rate END
        +coalesce(r.output_tokens,0)*o.output_rate) ELSE NULL END;
    debit:=least(held_amount,coalesce(value,0))::bigint;
    IF value>held_amount OR r.input_tokens>r.input_bound OR r.output_tokens>r.output_bound THEN
        PERFORM FROM relay_hosted.text_offers WHERE id=o.id FOR UPDATE;
        INSERT INTO relay_hosted.text_fences(offer_id) VALUES(o.id) ON CONFLICT DO NOTHING;
    END IF;
    UPDATE relay_hosted.text_requests SET execution=next_execution,accounting=CASE WHEN value IS NULL THEN 'released' ELSE 'settled' END,
        basis=evidence,valued=value,charged=debit,closed_at=clock_timestamp() WHERE id=r.id AND closed_at IS NULL RETURNING * INTO STRICT r;
    UPDATE relay_hosted.wallets SET available_micro_usd=available_micro_usd+(held_amount-debit),reserved_micro_usd=reserved_micro_usd-held_amount,
        revision=revision+1 WHERE workspace_id=r.workspace_id AND reserved_micro_usd>=held_amount;
    IF NOT FOUND THEN RAISE EXCEPTION 'reservation projection mismatch'; END IF;
    PERFORM relay_hosted.record_request_journal(r,'request_close');
END $$;

CREATE FUNCTION relay_hosted.finish_text(oid uuid,rid uuid,aid uuid,outcome text,inputs bigint,cached bigint,outputs bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.text_requests; o relay_hosted.text_offers; final_usage boolean; priceable boolean;
BEGIN
    -- Same wallet -> offer -> request order as reserve. The permit never takes a wallet lock.
    PERFORM FROM relay_hosted.wallets w JOIN relay_hosted.text_requests x ON x.workspace_id=w.workspace_id
        WHERE x.id=rid AND x.attempt_id=aid AND x.offer_id=oid FOR NO KEY UPDATE OF w;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=oid FOR NO KEY UPDATE;
    SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid FOR UPDATE;
    IF r.closed_at IS NOT NULL THEN RETURN r.accounting; END IF;
    IF r.execution='reserved' THEN
        IF outcome='not_sent' OR (outcome='deadline' AND r.claim_by<=clock_timestamp()) THEN
            PERFORM relay_hosted.close_text(r,'failed','not_sent'); RETURN 'released'; END IF;
        RETURN 'held';
    END IF;
    IF outcome='deadline' AND r.close_by>clock_timestamp() THEN RETURN r.accounting; END IF;
    IF outcome NOT IN ('completed','incomplete','unknown','not_sent','deadline') OR outcome IS NULL THEN RETURN 'invalid'; END IF;
    IF outcome IN ('completed','incomplete') AND r.close_by>clock_timestamp() AND r.execution IN ('dispatched','uncertain') THEN
        UPDATE relay_hosted.text_requests SET execution=outcome,input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
            WHERE id=rid RETURNING * INTO r;
    ELSIF r.execution='dispatched' THEN
        UPDATE relay_hosted.text_requests SET execution='uncertain',accounting='pending' WHERE id=rid RETURNING * INTO r;
    END IF;
    final_usage:=r.input_tokens IS NOT NULL AND (r.cached_tokens IS NOT NULL OR o.input_rate=o.cached_rate) AND r.output_tokens IS NOT NULL;
    priceable:=r.output_tokens IS NOT NULL OR (r.input_tokens IS NOT NULL AND (o.input_rate=o.cached_rate OR r.cached_tokens IS NOT NULL));
    IF final_usage THEN PERFORM relay_hosted.close_text(r,r.execution,'provider_final'); RETURN 'settled'; END IF;
    IF r.close_by<=clock_timestamp() THEN
        PERFORM relay_hosted.close_text(r,r.execution,CASE WHEN priceable THEN 'provider_partial' ELSE 'policy_waiver' END);
        RETURN CASE WHEN priceable THEN 'settled' ELSE 'released' END;
    END IF;
    RETURN 'pending';
END $$;

CREATE FUNCTION relay_hosted.due_text()
RETURNS TABLE(oid uuid,rid uuid,aid uuid) LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT r.offer_id,r.id,r.attempt_id FROM relay_hosted.text_requests r WHERE r.closed_at IS NULL AND
        ((r.execution='reserved' AND r.claim_by<=statement_timestamp()) OR r.close_by<=statement_timestamp())
        ORDER BY r.close_by,r.id LIMIT 32
$$;

CREATE FUNCTION relay_hosted.text_receipt(r relay_hosted.text_requests)
RETURNS jsonb LANGUAGE sql STABLE STRICT SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('request_id',r.id,'key_id',r.key_id,'model',o.model,'tariff_revision',o.revision::text,
        'started_at_s',floor(extract(epoch FROM r.started_at))::bigint,'execution',r.execution,'accounting',r.accounting,'basis',r.basis,
        'input_tokens',r.input_tokens::text,'cached_tokens',r.cached_tokens::text,'output_tokens',r.output_tokens::text,
        'total_tokens',(r.input_tokens::numeric+r.output_tokens)::text,
        'currency','USD','precision',6,'hold_micro_usd',r.held::text,'charge_micro_usd',r.charged::text,
        'wallet_receipt_id',(SELECT j.id FROM relay_hosted.journal_transactions j WHERE j.request_id=r.id AND j.operation_kind='request_close'))
    FROM relay_hosted.text_offers o WHERE o.id=r.offer_id
$$;
CREATE FUNCTION relay_hosted.key_text_status(origin text,kid uuid,proof bytea,rid uuid)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT relay_hosted.text_receipt(r) FROM relay_hosted.text_requests r
        WHERE r.id=rid AND r.key_id=kid AND r.workspace_id=relay_hosted.qualified_key_workspace(origin,kid,proof)
$$;
REVOKE ALL ON FUNCTION relay_hosted.text_offer_config(uuid,text),relay_hosted.lock_text_authority(text,uuid,uuid,bytea),
    relay_hosted.reserve_text(text,uuid,uuid,bytea,bytea,bytea,bytea,text,integer,integer),
    relay_hosted.claim_text(text,uuid,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer),
    relay_hosted.close_text(relay_hosted.text_requests,text,text),relay_hosted.finish_text(uuid,uuid,uuid,text,bigint,bigint,bigint),
    relay_hosted.due_text(),relay_hosted.text_receipt(relay_hosted.text_requests),relay_hosted.key_text_status(text,uuid,bytea,uuid) FROM PUBLIC;
