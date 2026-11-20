-- Keyset pages. Request history uses the start time; wallet history uses posting time.
CREATE FUNCTION relay_hosted.customer_history(origin text,provider uuid,binding bytea,token bytea,kind text,
    hours integer,before_id uuid,target uuid)
RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$
DECLARE space uuid; anchor timestamptz:=statement_timestamp(); since_at timestamptz; cursor_at timestamptz; rows jsonb;
BEGIN
    SELECT a.workspace_id INTO space FROM relay_hosted.customer_account(origin,provider,binding,token) a;
    IF NOT FOUND THEN RETURN NULL; END IF;
    IF hours NOT IN (24,168,720) OR hours IS NULL THEN RETURN NULL; END IF;
    since_at:=anchor-hours*interval '1 hour';
    IF kind='usage' THEN
        IF before_id IS NOT NULL THEN
            SELECT started_at INTO cursor_at FROM relay_hosted.text_requests WHERE workspace_id=space AND id=before_id;
            IF NOT FOUND THEN RETURN '[]'::jsonb; END IF;
        END IF;
        SELECT coalesce(jsonb_agg(p.receipt ORDER BY p.ts DESC,p.id DESC),'[]') INTO rows FROM
            (SELECT r.started_at AS ts,r.id,relay_hosted.text_receipt(r) AS receipt FROM relay_hosted.text_requests r
            WHERE r.workspace_id=space AND (target IS NULL OR r.id=target)
                AND (target IS NOT NULL OR (r.started_at>=since_at AND r.started_at<=anchor))
                AND (before_id IS NULL OR (r.started_at,r.id)<(cursor_at,before_id))
            ORDER BY r.started_at DESC,r.id DESC LIMIT 33) p;
    ELSIF kind='wallet' THEN
        IF before_id IS NOT NULL THEN
            SELECT created_at INTO cursor_at FROM relay_hosted.journal_transactions WHERE workspace_id=space AND id=before_id;
            IF NOT FOUND THEN RETURN '[]'::jsonb; END IF;
        END IF;
        SELECT coalesce(jsonb_agg(p.entry ORDER BY p.ts DESC,p.id DESC),'[]') INTO rows FROM
            (SELECT j.created_at AS ts,j.id,jsonb_build_object('id',j.id,'kind',j.operation_kind,'request_id',j.request_id,
                'posted_at_s',floor(extract(epoch FROM j.created_at))::bigint,'amount_micro_usd',CASE j.operation_kind
                    WHEN 'payment_credit' THEN i.credit_micro_usd WHEN 'request_hold' THEN r.held ELSE r.charged END::text) AS entry
            FROM relay_hosted.journal_transactions j LEFT JOIN relay_hosted.payment_intents i ON i.id=j.payment_id
            LEFT JOIN relay_hosted.text_requests r ON r.id=j.request_id
            WHERE j.workspace_id=space AND (target IS NULL OR j.id=target)
                AND (target IS NOT NULL OR (j.created_at>=since_at AND j.created_at<=anchor))
                AND (before_id IS NULL OR (j.created_at,j.id)<(cursor_at,before_id))
            ORDER BY j.created_at DESC,j.id DESC LIMIT 33) p;
    END IF;
    RETURN rows;
END $$;
REVOKE ALL ON FUNCTION relay_hosted.customer_history(text,uuid,bytea,bytea,text,integer,uuid,uuid) FROM PUBLIC;
