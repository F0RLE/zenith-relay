-- JSON evidence does not authorize SSE. A stream check belongs to one immutable inventory binding.
CREATE TABLE relay_hosted.text_stream_checks (
    inventory_id uuid PRIMARY KEY REFERENCES relay_hosted.text_inventory,
    operation_id uuid NOT NULL UNIQUE REFERENCES relay_hosted.text_check_attempts,
    checked_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER sealed_text_stream_check BEFORE UPDATE OR DELETE ON relay_hosted.text_stream_checks
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE OR REPLACE FUNCTION relay_hosted.text_probe_in_flight(source uuid)
RETURNS boolean LANGUAGE sql VOLATILE STRICT SET search_path=pg_catalog AS $$
    SELECT EXISTS(SELECT FROM relay_hosted.text_check_attempts j
        WHERE j.source_id=source AND j.claimed AND j.expires_at>clock_timestamp()
        AND NOT EXISTS(SELECT FROM relay_hosted.text_inventory i WHERE i.id=j.id)
        AND NOT EXISTS(SELECT FROM relay_hosted.text_stream_checks c WHERE c.operation_id=j.id))
$$;

CREATE FUNCTION relay_hosted.text_stream_check(origin text,scope uuid,op uuid,inventory uuid,step text)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE i relay_hosted.text_inventory; profile jsonb; result text; prior uuid;
BEGIN
    IF step NOT IN ('prepare','claim','record') OR op=inventory
        OR op='00000000-0000-0000-0000-000000000000' THEN RETURN NULL; END IF;
    PERFORM FROM relay_hosted.service_control s JOIN relay_hosted.installations h ON h.id=s.installation_id
        WHERE h.public_origin=origin AND s.qualification_scope_ref=scope AND s.mode='qualification' FOR UPDATE OF s;
    IF NOT FOUND THEN RETURN NULL; END IF;
    PERFORM pg_advisory_xact_lock_shared(98653249,1);
    SELECT original.* INTO i FROM relay_hosted.current_text_inventory c
        JOIN relay_hosted.text_inventory original ON original.id=c.id
        JOIN relay_hosted.configuration_revisions q ON q.id=c.scope_id
        WHERE c.id=inventory AND c.scope_id=scope AND c.public_origin=origin AND q.expires_at>clock_timestamp()
            AND NOT relay_hosted.text_inventory_fenced(c.id);
    IF NOT FOUND THEN RETURN NULL; END IF;
    SELECT operation_id INTO prior FROM relay_hosted.text_stream_checks WHERE inventory_id=i.id;
    IF FOUND THEN RETURN CASE WHEN prior=op THEN jsonb_build_object('state','done') END; END IF;
    profile:=jsonb_build_object('id',op,'scope',scope,'source',i.source_id,'provider_revision',i.provider_revision,
        'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,'stream_inventory',i.id);
    IF step='record' THEN
        PERFORM FROM relay_hosted.text_check_attempts j WHERE j.id=op AND j.scope_id=scope AND j.source_id=i.source_id
            AND j.digest=sha256(convert_to(profile::text,'UTF8')) AND j.claimed AND j.expires_at>clock_timestamp() FOR UPDATE;
        IF NOT FOUND THEN RETURN NULL; END IF;
        INSERT INTO relay_hosted.text_stream_checks(inventory_id,operation_id) VALUES(i.id,op);
        RETURN jsonb_build_object('state','done');
    END IF;
    result:=relay_hosted.text_check_step(origin,scope,op,i.source_id,i.provider_revision,i.proxy_revision,i.uses_proxy,profile,step);
    IF result='prepared' THEN
        RETURN jsonb_build_object('state','prepared','source',i.source_id,'endpoint',i.base_url,'model',i.model,
            'provider_revision',i.provider_revision,'proxy_revision',i.proxy_revision,'uses_proxy',i.uses_proxy,
            'direct_ipv4',host(i.direct_ipv4),'max_input_bytes',i.max_input_bytes,'max_output_tokens',i.max_output_tokens,
            'tokens_per_byte',i.tokens_per_byte,'overhead_tokens',i.overhead_tokens);
    END IF;
    RETURN jsonb_build_object('state',result);
END $$;
REVOKE ALL ON FUNCTION relay_hosted.text_stream_check(text,uuid,uuid,uuid,text) FROM PUBLIC;

ALTER TABLE relay_hosted.text_requests ADD COLUMN streaming boolean NOT NULL DEFAULT false;
CREATE TRIGGER immutable_text_transport BEFORE UPDATE OF streaming ON relay_hosted.text_requests
    FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();

CREATE FUNCTION relay_hosted.reserve_text(origin text,oid uuid,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer,wants_stream boolean)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE o relay_hosted.text_offers; r relay_hosted.text_requests; balance bigint; stamp timestamptz; sized integer; supply uuid[];
BEGIN
    SELECT * INTO o FROM relay_hosted.text_offers WHERE id=oid;
    space:=relay_hosted.qualified_key_workspace(origin,kid,proof);
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

CREATE OR REPLACE FUNCTION relay_hosted.reserve_text(origin text,oid uuid,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT * FROM relay_hosted.reserve_text($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,false)
$$;

CREATE FUNCTION relay_hosted.reserve_published_text(origin text,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer,wants_stream boolean)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint,offer uuid,tariff bigint)
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE oid uuid; v bigint; workspace uuid;
BEGIN
    workspace:=relay_hosted.qualified_key_workspace(origin,kid,proof);
    IF workspace IS NULL THEN result:='denied'; RETURN NEXT; RETURN; END IF;
    PERFORM FROM relay_hosted.wallets w WHERE w.workspace_id=workspace FOR NO KEY UPDATE;
    SELECT l.id,l.revision INTO oid,v FROM relay_hosted.active_text_listings l
        JOIN relay_hosted.text_model_rules r ON r.id=l.item AND r.last_offer=l.id AND r.enabled
        JOIN relay_hosted.qualification_subjects q ON q.scope_id=l.scope_id AND q.workspace_id=workspace
        WHERE l.model=requested_model AND l.public_origin=origin FOR SHARE OF r;
    RETURN QUERY SELECT x.*,oid,v FROM relay_hosted.reserve_text(origin,oid,kid,proof,op,body_tag,claim_tag,
        requested_model,input_bytes,output_cap,wants_stream) x;
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.reserve_published_text(origin text,kid uuid,proof bytea,op bytea,body_tag bytea,claim_tag bytea,
    requested_model text,input_bytes integer,output_cap integer)
RETURNS TABLE(result text,rid uuid,attempt uuid,space uuid,epoch bigint,held bigint,offer uuid,tariff bigint)
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT * FROM relay_hosted.reserve_published_text($1,$2,$3,$4,$5,$6,$7,$8,$9,false)
$$;

REVOKE ALL ON FUNCTION relay_hosted.reserve_text(text,uuid,uuid,bytea,bytea,bytea,bytea,text,integer,integer,boolean),
    relay_hosted.reserve_published_text(text,uuid,bytea,bytea,bytea,bytea,text,integer,integer,boolean) FROM PUBLIC;
