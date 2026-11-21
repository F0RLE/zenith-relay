CREATE OR REPLACE FUNCTION relay_hosted.capture_text_stream_usage(oid uuid,rid uuid,aid uuid,inputs bigint,cached bigint,outputs bigint)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE prior relay_hosted.text_requests;
BEGIN
    IF inputs<0 OR outputs<0 OR (cached IS NOT NULL AND (inputs IS NULL OR cached NOT BETWEEN 0 AND inputs))
        OR (inputs IS NULL AND outputs IS NULL) THEN RETURN; END IF;
    SELECT * INTO prior FROM relay_hosted.text_requests r
        WHERE (r.id,r.attempt_id,r.offer_id)=(rid,aid,oid) AND r.streaming AND r.execution IN ('dispatched','uncertain')
            AND r.closed_at IS NULL FOR NO KEY UPDATE;
    IF NOT FOUND OR prior.close_by<=clock_timestamp()
        OR EXISTS(SELECT FROM relay_hosted.text_terminal_evidence e WHERE e.request_id=rid) THEN RETURN; END IF;
    -- Input and cache must come from the same snapshot. Output can advance independently.
    IF inputs IS NULL OR inputs<prior.input_tokens
        OR (prior.cached_tokens IS NOT NULL AND (cached IS NULL OR cached<prior.cached_tokens)) THEN
        inputs:=prior.input_tokens;
        cached:=prior.cached_tokens;
    END IF;
    outputs:=greatest(outputs,prior.output_tokens);
    UPDATE relay_hosted.text_requests r SET input_tokens=inputs,cached_tokens=cached,output_tokens=outputs,accounting='pending'
        WHERE r.id=rid AND (r.input_tokens,r.cached_tokens,r.output_tokens) IS DISTINCT FROM (inputs,cached,outputs);
END $$;
