-- A direct HTTPS observation belongs to the reviewed socket, not just the URL.
ALTER TABLE relay_hosted.text_inventory ADD COLUMN direct_ipv4 inet CHECK (
    direct_ipv4 IS NULL OR (NOT uses_proxy AND family(direct_ipv4)=4
        AND masklen(direct_ipv4)=32 AND base_url LIKE 'https://%')
);

DROP FUNCTION relay_hosted.record_text_check(text,uuid,uuid,uuid,bigint,bigint,boolean,text,text,text,integer,integer,integer,integer,integer,integer);
CREATE FUNCTION relay_hosted.record_text_check(origin text, scope uuid, item uuid, source uuid, provider_rev bigint,
    proxy_rev bigint, proxied boolean, endpoint text, model_id text, label text, input_limit integer, output_limit integer,
    multiplier integer, overhead integer, closure integer, lifetime integer, pin inet)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE iid uuid; expiry timestamptz; stamp timestamptz; prev relay_hosted.text_inventory; expected bytea; profile jsonb;
BEGIN
    IF pin IS NULL THEN
        IF endpoint NOT LIKE 'http://%' THEN RETURN false; END IF;
    ELSIF family(pin)<>4 OR masklen(pin)<>32 OR proxied OR endpoint NOT LIKE 'https://%' THEN RETURN false;
    END IF;
    profile:=jsonb_build_object('id',item,'scope',scope,'source',source,'source_label',label,'endpoint',endpoint,
        'model',model_id,'provider_revision',provider_rev,'proxy_revision',proxy_rev,'uses_proxy',proxied,
        'max_input_bytes',input_limit,'max_output_tokens',output_limit,'tokens_per_byte',multiplier,'overhead_tokens',overhead,
        'close_after_s',closure,'valid_for_s',lifetime);
    IF pin IS NOT NULL THEN profile:=profile||jsonb_build_object('direct_ipv4',host(pin)); END IF;
    expected:=sha256(convert_to(profile::text,'UTF8'));
    SELECT h.id,q.expires_at INTO iid,expiry FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
        JOIN relay_hosted.configuration_revisions q ON q.id=s.qualification_scope_ref
        WHERE h.public_origin=origin AND s.mode='qualification' AND q.id=scope FOR UPDATE OF s;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    stamp:=clock_timestamp();
    IF iid IS NULL OR expiry<=stamp OR lifetime NOT BETWEEN 60 AND 86400 OR NOT EXISTS(
        SELECT FROM relay_hosted.text_check_attempts j WHERE j.id=item AND j.scope_id=scope AND j.source_id=source
            AND j.digest=expected AND j.claimed AND j.expires_at>stamp)
        OR NOT relay_hosted.text_source_matches(iid,source,provider_rev,proxy_rev,proxied) THEN RETURN false; END IF;
    SELECT * INTO prev FROM relay_hosted.text_inventory WHERE id=item;
    IF FOUND THEN RETURN (prev.scope_id,prev.source_id,prev.provider_revision,prev.proxy_revision,prev.uses_proxy,prev.base_url,
        prev.model,prev.source_label,prev.max_input_bytes,prev.max_output_tokens,prev.tokens_per_byte,prev.overhead_tokens,prev.close_after_s)
        =(scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,input_limit,output_limit,multiplier,overhead,closure)
        AND prev.direct_ipv4 IS NOT DISTINCT FROM pin AND prev.valid_until>stamp; END IF;
    IF (SELECT count(*) FROM relay_hosted.text_inventory WHERE scope_id=scope)>=128 THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_inventory(id,scope_id,source_id,provider_revision,proxy_revision,uses_proxy,base_url,
        model,source_label,max_input_bytes,max_output_tokens,tokens_per_byte,overhead_tokens,close_after_s,checked_at,valid_until,direct_ipv4)
        VALUES(item,scope,source,provider_rev,proxy_rev,proxied,endpoint,model_id,label,input_limit,output_limit,multiplier,overhead,
            closure,stamp,least(expiry,stamp+lifetime*interval '1 second'),pin);
    INSERT INTO relay_hosted.text_model_rules(id,scope_id,model) VALUES(item,scope,model_id);
    RETURN true;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.record_text_check(text,uuid,uuid,uuid,bigint,bigint,boolean,text,text,text,integer,integer,integer,integer,integer,integer,inet) FROM PUBLIC;

CREATE OR REPLACE FUNCTION relay_hosted.text_offer_config(oid uuid,origin text)
RETURNS jsonb LANGUAGE sql STABLE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT jsonb_build_object('item',i.id,'source',i.source_id,'model',o.model,'endpoint',o.base_url,
        'provider_revision',i.provider_revision,'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,
        'input_bytes',o.max_input_bytes,'direct_ipv4',host(t.direct_ipv4))
    FROM relay_hosted.text_offers o JOIN relay_hosted.current_text_inventory i ON i.id=o.inventory_id
    JOIN relay_hosted.text_inventory t ON t.id=i.id
    JOIN relay_hosted.text_model_rules r ON r.id=i.id WHERE o.id=oid AND i.public_origin=origin AND r.enabled AND r.admitted
        AND o.revision>=r.first_live_revision
$$;
