-- One validated native JSON terminal per attempt. Do not store the response body.
CREATE TABLE relay_hosted.text_terminal_evidence (
    request_id uuid PRIMARY KEY REFERENCES relay_hosted.text_requests,
    outcome text NOT NULL CHECK (outcome IN ('completed','incomplete')),
    input_tokens bigint CHECK (input_tokens>=0),
    cached_tokens bigint CHECK (cached_tokens IS NULL OR
        (input_tokens IS NOT NULL AND cached_tokens BETWEEN 0 AND input_tokens)),
    output_tokens bigint CHECK (output_tokens>=0),
    observed_at timestamptz NOT NULL,
    applied_at timestamptz
);
CREATE TRIGGER sealed_text_terminal BEFORE UPDATE OF request_id,outcome,input_tokens,cached_tokens,output_tokens,observed_at
    OR DELETE ON relay_hosted.text_terminal_evidence FOR EACH ROW EXECUTE FUNCTION relay_hosted.reject_financial_rewrite();
CREATE INDEX unapplied_text_terminal ON relay_hosted.text_terminal_evidence(observed_at,request_id) WHERE applied_at IS NULL;

CREATE FUNCTION relay_hosted.capture_text_terminal(oid uuid,rid uuid,aid uuid,kind text,inputs bigint,cached bigint,outputs bigint)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE stamp timestamptz:=clock_timestamp();
BEGIN
    IF kind IS NULL OR kind NOT IN ('completed','incomplete') OR inputs<0 OR outputs<0
        OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs)) THEN RETURN false; END IF;
    -- Serialize capture with closure, but do not wait on the wallet or append any postings here.
    PERFORM FROM relay_hosted.text_requests r WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid)
        AND r.claimed_at IS NOT NULL FOR NO KEY UPDATE;
    IF NOT FOUND THEN RETURN false; END IF;
    INSERT INTO relay_hosted.text_terminal_evidence(request_id,outcome,input_tokens,cached_tokens,output_tokens,observed_at)
        VALUES(rid,kind,inputs,cached,outputs,stamp) ON CONFLICT (request_id) DO NOTHING;
    RETURN FOUND OR EXISTS(SELECT FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=rid
        AND (e.outcome,e.input_tokens,e.cached_tokens,e.output_tokens) IS NOT DISTINCT FROM (kind,inputs,cached,outputs));
END $$;

CREATE OR REPLACE FUNCTION relay_hosted.finish_text(oid uuid,rid uuid,aid uuid,outcome text,inputs bigint,cached bigint,outputs bigint)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE r relay_hosted.text_requests; o relay_hosted.text_offers; terminal relay_hosted.text_terminal_evidence;
    incoming_value numeric; stamp timestamptz; before_cutoff boolean; expired boolean; priceable boolean;
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
        -- Apply the captured terminal before accepting a separate usage delivery.
        IF outcome IN ('completed','incomplete') THEN RETURN 'terminal_pending'; END IF;
        outcome:=terminal.outcome;
        inputs:=terminal.input_tokens;
        cached:=terminal.cached_tokens;
        outputs:=terminal.output_tokens;
        UPDATE relay_hosted.text_terminal_evidence SET applied_at=stamp WHERE request_id=rid;
    END IF;
    before_cutoff:=coalesce(terminal.observed_at,stamp)<r.close_by;
    expired:=stamp>=r.close_by;
    IF r.claimed_at IS NOT NULL AND outcome IN ('completed','incomplete') THEN
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
    IF before_cutoff AND outcome IN ('completed','incomplete')
        AND (r.execution IN ('dispatched','uncertain') OR incoming_value IS NOT NULL) THEN
        UPDATE relay_hosted.text_requests SET execution=CASE WHEN r.execution IN ('completed','incomplete')
            THEN r.execution ELSE outcome END,input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
            WHERE id=rid RETURNING * INTO r;
    ELSIF r.execution='dispatched' THEN
        UPDATE relay_hosted.text_requests SET execution='uncertain',accounting='pending' WHERE id=rid RETURNING * INTO r;
    END IF;
    IF relay_hosted.text_final_value(r.input_tokens,r.cached_tokens,r.output_tokens,o.input_rate,o.cached_rate,o.output_rate)
        IS NOT NULL THEN
        PERFORM relay_hosted.close_text(r,r.execution,'provider_final');
        RETURN 'settled';
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

CREATE FUNCTION relay_hosted.reconcile_text_attempt(oid uuid,rid uuid,aid uuid,action text)
RETURNS text LANGUAGE sql VOLATILE STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT CASE WHEN $4 IN ('deadline','not_sent','unknown')
        THEN relay_hosted.finish_text($1,$2,$3,$4,NULL,NULL,NULL) ELSE 'invalid' END
$$;
REVOKE ALL ON FUNCTION relay_hosted.capture_text_terminal(uuid,uuid,uuid,text,bigint,bigint,bigint),
    relay_hosted.reconcile_text_attempt(uuid,uuid,uuid,text) FROM PUBLIC;

CREATE OR REPLACE FUNCTION relay_hosted.due_text()
RETURNS TABLE(oid uuid,rid uuid,aid uuid) LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog AS $$
    SELECT q.offer_id,q.id,q.attempt_id FROM (
        SELECT r.offer_id,r.id,r.attempt_id,e.observed_at AS due_at
        FROM relay_hosted.text_terminal_evidence e JOIN relay_hosted.text_requests r ON r.id=e.request_id
        WHERE e.applied_at IS NULL
        UNION ALL
        SELECT r.offer_id,r.id,r.attempt_id,r.close_by FROM relay_hosted.text_requests r
        WHERE r.closed_at IS NULL AND
            ((r.execution='reserved' AND r.claim_by<=statement_timestamp()) OR r.close_by<=statement_timestamp())
        UNION ALL
        SELECT r.offer_id,r.id,r.attempt_id,c.observed_at FROM relay_hosted.text_customer_credits c
        JOIN relay_hosted.text_requests r ON r.id=c.request_id JOIN relay_hosted.wallets w ON w.workspace_id=r.workspace_id
        WHERE w.available_micro_usd<=9223372036854775807-w.reserved_micro_usd-c.amount_micro_usd AND NOT EXISTS (
            SELECT FROM relay_hosted.journal_transactions j WHERE j.request_id=r.id AND j.operation_kind='request_credit')
    ) q GROUP BY q.offer_id,q.id,q.attempt_id ORDER BY min(q.due_at),q.id LIMIT 32
$$;
