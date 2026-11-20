-- The scope's named verified testers are the approved audience. New logins inherit
-- an already published rule; inventory and draft rows alone grant nothing.
-- An over-bound result on an older tariff must fence the same inventory on
-- newer tariffs too. Serialize the fence with physical permits, not with HTTP.
CREATE FUNCTION relay_hosted.pin_text_fence() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('text-source:'||o.source_id::text,0))
        FROM relay_hosted.text_offers o WHERE o.id=NEW.offer_id;
    RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.pin_text_fence() FROM PUBLIC;
CREATE TRIGGER pin_text_fence BEFORE INSERT ON relay_hosted.text_fences
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.pin_text_fence();
CREATE OR REPLACE FUNCTION relay_hosted.lock_text_authority(origin text, oid uuid, kid uuid, proof bytea)
RETURNS uuid LANGUAGE plpgsql STRICT SET search_path=pg_catalog AS $$
DECLARE space uuid; until_at timestamptz; iid uuid; item relay_hosted.text_inventory;
BEGIN
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT i.* INTO item FROM relay_hosted.text_offers o JOIN relay_hosted.text_inventory i ON i.id=o.inventory_id WHERE o.id=oid;
    IF NOT FOUND THEN RETURN NULL; END IF;
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('text-source:'||item.source_id::text,0));
    SELECT k.workspace_id,h.id,least(k.expires_at,q.expires_at,item.valid_until) INTO space,iid,until_at
        FROM relay_hosted.customer_keys k JOIN relay_hosted.configuration_revisions q ON q.id=k.scope_id
        JOIN relay_hosted.service_control s ON s.qualification_scope_ref=q.id
        JOIN relay_hosted.installations h ON h.id=s.installation_id
        JOIN relay_hosted.text_model_rules r ON r.scope_id=q.id AND r.id=item.id
        JOIN relay_hosted.text_offers o ON o.id=oid AND o.revision>=r.first_live_revision
        JOIN relay_hosted.workspaces w ON w.id=k.workspace_id
        JOIN relay_hosted.qualification_subjects a ON a.scope_id=q.id AND a.workspace_id=w.id
        JOIN relay_hosted.external_identities e ON e.provider_id=q.provider_id AND e.subject_digest=a.subject_digest
        JOIN relay_hosted.users u ON u.id=e.user_id AND u.id=w.owner_user_id
        WHERE h.public_origin=origin AND s.mode='qualification' AND r.admitted AND r.enabled AND k.id=kid AND k.verifier=proof
            AND k.installation_epoch=h.authorization_epoch AND k.user_epoch=u.authorization_epoch AND k.revoked_at IS NULL
            AND e.revoked_at IS NULL AND u.disabled_at IS NULL FOR SHARE OF k,q,s,h,r,w,a,e,u;
    IF NOT FOUND OR until_at<=clock_timestamp() OR relay_hosted.text_probe_in_flight(item.source_id) OR NOT relay_hosted.text_source_matches(iid,item.source_id,item.provider_revision,
        item.proxy_revision,item.uses_proxy) OR EXISTS(SELECT FROM relay_hosted.text_fences f JOIN relay_hosted.text_offers o ON o.id=f.offer_id
            WHERE o.inventory_id=item.id) THEN RETURN NULL; END IF;
    RETURN space;
END $$;
CREATE OR REPLACE FUNCTION relay_hosted.text_offer_config(oid uuid,origin text)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('item',i.id,'source',i.source_id,'model',o.model,'endpoint',o.base_url,
        'provider_revision',i.provider_revision,'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,'input_bytes',o.max_input_bytes)
    FROM relay_hosted.text_offers o JOIN relay_hosted.current_text_inventory i ON i.id=o.inventory_id
    JOIN relay_hosted.text_model_rules r ON r.id=i.id WHERE o.id=oid AND i.public_origin=origin AND r.enabled AND r.admitted
        AND o.revision>=r.first_live_revision
$$;
CREATE FUNCTION relay_hosted.reserve_published_text(origin text,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint,offer uuid,tariff bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE oid uuid; v bigint; workspace uuid;
BEGIN
    workspace:=relay_hosted.qualified_key_workspace(origin,kid,proof);
    IF workspace IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    -- Keep the wallet first in the lock order; serialize replays with the original admission.
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=workspace FOR NO KEY UPDATE;
    SELECT l.id,l.revision INTO oid,v FROM relay_hosted.active_text_listings l
        JOIN relay_hosted.text_model_rules r ON r.id=l.item AND r.last_offer=l.id AND r.enabled
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id AND q.workspace_id=workspace
        WHERE l.model=requested_model AND l.public_origin=origin FOR SHARE OF r;
    -- Even when withdrawn, an authenticated replay can recover its original logical ID.
    RETURN QUERY SELECT x.*,oid,v FROM relay_hosted.reserve_text(origin,oid,kid,proof,op,body_tag,claim_tag,
        requested_model,input_bytes,output_cap) x;
END $$;
CREATE FUNCTION relay_hosted.claim_published_text(origin text,rid uuid,aid uuid,space uuid,kid uuid,source uuid,model text,
    auth_rev bigint,tariff_rev bigint,bound bigint,tag bytea,input_cap integer,output_cap integer)
RETURNS boolean LANGUAGE sql VOLATILE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT coalesce((SELECT relay_hosted.claim_text(origin,r.offer_id,rid,aid,space,kid,source,model,auth_rev,tariff_rev,bound,tag,input_cap,output_cap)
        FROM relay_hosted.text_requests r WHERE r.id=rid AND r.attempt_id=aid AND r.workspace_id=space AND r.key_id=kid),false)
$$;
REVOKE ALL ON FUNCTION relay_hosted.reserve_published_text(text,uuid,bytea,bytea,bytea,bytea,text,integer,integer),
    relay_hosted.claim_published_text(text,uuid,uuid,uuid,uuid,uuid,text,bigint,bigint,bigint,bytea,integer,integer) FROM PUBLIC;
