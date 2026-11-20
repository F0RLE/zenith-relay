-- A stream can report usage before it terminates. Such counts do not prove completion.
ALTER TABLE relay_hosted.text_terminal_evidence DROP CONSTRAINT text_terminal_evidence_outcome_check,
    ADD CHECK (outcome IN ('completed','incomplete','failed'));

CREATE FUNCTION relay_hosted.capture_text_stream_usage(oid uuid,rid uuid,aid uuid,inputs bigint,cached bigint,outputs bigint)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF inputs<0 OR outputs<0 OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs))
        OR (inputs IS NULL AND outputs IS NULL) THEN RETURN; END IF;
    UPDATE relay_hosted.text_requests r SET input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
        WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid) AND r.streaming AND r.execution IN ('dispatched','uncertain')
            AND r.closed_at IS NULL AND r.close_by>clock_timestamp()
            AND (r.input_tokens IS NULL OR inputs>=r.input_tokens)
            AND (r.cached_tokens IS NULL OR cached>=r.cached_tokens)
            AND (r.output_tokens IS NULL OR outputs>=r.output_tokens)
            AND (r.input_tokens,r.cached_tokens,r.output_tokens) IS DISTINCT FROM (inputs,cached,outputs)
            AND NOT EXISTS(SELECT FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=r.id);
END $$;
REVOKE ALL ON FUNCTION relay_hosted.capture_text_stream_usage(uuid,uuid,uuid,bigint,bigint,bigint) FROM PUBLIC;

CREATE OR REPLACE FUNCTION relay_hosted.capture_text_terminal(oid uuid,rid uuid,aid uuid,kind text,inputs bigint,cached bigint,outputs bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=clock_timestamp();
BEGIN
    IF kind IS NULL OR kind NOT IN ('completed','incomplete','failed') OR inputs<0 OR outputs<0
        OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs)) THEN RETURN false; END IF;
    PERFORM FROM relay_hosted.text_requests r WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid)
        AND r.claimed_at IS NOT NULL AND (kind<>'failed' OR r.streaming) FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_terminal_evidence(request_id,outcome,input_tokens,cached_tokens,output_tokens,observed_at)
        VALUES(rid,kind,inputs,cached,outputs,stamp) ON CONFLICT (request_id) DO NOTHING;
    RETURN FOUND OR EXISTS(SELECT FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=rid
        AND (e.outcome,e.input_tokens,e.cached_tokens,e.output_tokens) IS NOT DISTINCT FROM (kind,inputs,cached,outputs));
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.finish_text(oid uuid,rid uuid,aid uuid,outcome text,inputs bigint,cached bigint,outputs bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.text_requests; o relay_hosted.text_offers; terminal relay_hosted.text_terminal_evidence;
    incoming_value numeric; stamp timestamptz; before_cutoff boolean; expired boolean; priceable boolean; use_input boolean;
BEGIN
    IF outcome IS NULL OR outcome NOT IN ('completed','incomplete','unknown','not_sent','deadline') THEN RETURN 'invalid'; END IF;
    PERFORM FROM relay_hosted.wallets w JOIN relay_hosted.text_requests x ON x.workspace_id=w.workspace_id
        WHERE x.id=rid AND x.attempt_id=aid AND x.offer_id=oid FOR NO KEY UPDATE OF w;
    IF NOT FOUND THEN RETURN 'missing'; END IF;
    SELECT * INTO STRICT o FROM relay_hosted.text_offers WHERE id=oid;
    SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid FOR UPDATE;
    SELECT * INTO terminal FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=rid AND e.applied_at IS NULL;
    stamp:=clock_timestamp();
    IF FOUND THEN
        IF outcome IN ('completed','incomplete') THEN RETURN 'terminal_pending'; END IF;
        outcome:=terminal.outcome;
        inputs:=terminal.input_tokens;
        cached:=terminal.cached_tokens;
        outputs:=terminal.output_tokens;
        UPDATE relay_hosted.text_terminal_evidence SET applied_at=stamp WHERE request_id=rid;
    END IF;
    before_cutoff:=coalesce(terminal.observed_at,stamp)<r.close_by;
    expired:=stamp>=r.close_by;
    IF r.claimed_at IS NOT NULL AND outcome IN ('completed','incomplete','failed') THEN
        IF inputs<0 OR outputs<0 OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs))
            THEN RETURN 'invalid'; END IF;
        incoming_value:=relay_hosted.text_final_value(inputs,cached,outputs,o.input_rate,o.cached_rate,o.output_rate);
        PERFORM relay_hosted.value_text_purchase(oid,aid,inputs,cached,outputs);
    END IF;
    IF r.closed_at IS NOT NULL THEN
        IF incoming_value IS NOT NULL THEN
            PERFORM relay_hosted.record_late_text_usage(r,inputs,cached,outputs,incoming_value);
        END IF;
        PERFORM relay_hosted.post_text_credit(r);
        RETURN r.accounting;
    END IF;
    IF r.execution='reserved' THEN
        IF outcome='not_sent' OR (outcome='deadline' AND r.claim_by<=stamp) THEN
            PERFORM relay_hosted.close_text(r,'failed','not_sent'); RETURN 'released'; END IF;
        RETURN 'held';
    END IF;
    IF outcome='deadline' AND NOT expired THEN RETURN r.accounting; END IF;
    IF before_cutoff AND outcome IN ('completed','incomplete','failed')
        AND (r.execution IN ('dispatched','uncertain') OR incoming_value IS NOT NULL) THEN
        -- Use an exact terminal as-is. For partial accounting, preserve an earlier priceable input pair
        -- when the terminal omits it. Never combine cache and input counts from different snapshots.
        use_input:=inputs IS NOT NULL AND (incoming_value IS NOT NULL OR cached IS NOT NULL OR inputs=0
            OR o.input_rate=o.cached_rate OR r.input_tokens IS NULL OR (r.input_tokens>0 AND r.cached_tokens IS NULL));
        UPDATE relay_hosted.text_requests SET execution=CASE WHEN r.execution IN ('completed','incomplete','failed')
            THEN r.execution ELSE outcome END,input_tokens=CASE WHEN use_input THEN inputs ELSE r.input_tokens END,
            cached_tokens=CASE WHEN use_input THEN cached ELSE r.cached_tokens END,
            output_tokens=coalesce(outputs,r.output_tokens),accounting='pending'
            WHERE id=rid RETURNING * INTO r;
        IF incoming_value IS NOT NULL THEN
            PERFORM relay_hosted.close_text(r,r.execution,'provider_final');
            RETURN 'settled';
        END IF;
    ELSIF r.execution='dispatched' THEN
        UPDATE relay_hosted.text_requests SET execution='uncertain',accounting='pending' WHERE id=rid RETURNING * INTO r;
    END IF;
    IF expired THEN
        priceable:=r.output_tokens IS NOT NULL OR (r.input_tokens IS NOT NULL
            AND (r.input_tokens=0 OR o.input_rate=o.cached_rate OR r.cached_tokens IS NOT NULL));
        PERFORM relay_hosted.close_text(r,r.execution,CASE WHEN priceable THEN 'provider_partial' ELSE 'policy_waiver' END);
        IF incoming_value IS NOT NULL THEN
            SELECT * INTO STRICT r FROM relay_hosted.text_requests WHERE id=rid;
            PERFORM relay_hosted.record_late_text_usage(r,inputs,cached,outputs,incoming_value);
            PERFORM relay_hosted.post_text_credit(r);
        END IF;
        RETURN CASE WHEN priceable THEN 'settled' ELSE 'released' END;
    END IF;
    RETURN 'pending';
END $$;
