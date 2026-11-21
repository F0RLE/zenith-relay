CREATE OR REPLACE FUNCTION relay_hosted.owner_text_rules(origin text,tag bytea)
RETURNS jsonb LANGUAGE plpgsql STRICT SECURITY DEFINER SET search_path=pg_catalog AS $$
BEGIN
    IF relay_hosted.read_admin_overview(origin,tag) IS NULL THEN RETURN NULL; END IF;
    RETURN (SELECT coalesce(jsonb_agg(jsonb_build_object('id',i.id,'model',i.model,
        'source_id',i.source_id,'source',i.source_label,'proxy',i.uses_proxy,
        'serving',relay_hosted.source_serving_view(h.id,i.source_id),
        'revision',r.revision::text,'admitted',r.admitted AND current_binding.active,'enabled',r.enabled,
        'ready',c.id IS NOT NULL AND NOT relay_hosted.text_inventory_fenced(i.id),
        'supply_ready',EXISTS(SELECT FROM relay_hosted.text_basis_supply(i,true) p WHERE p.rate_revision IS NOT NULL),
        'purchase',CASE WHEN p.item IS NOT NULL AND current_binding.active THEN
            jsonb_build_object('input',p.input_rate::text,'cached',p.cached_rate::text,'output',p.output_rate::text) END,
        'source_markup',jsonb_build_object('revision',coalesce(m.revision,0)::text,'percent',m.percent::text),
        'sale_rule',jsonb_build_object('revision',coalesce(q.revision,0)::text,'mode',coalesce(q.mode,'fixed'),
            'percent',q.percent::text,'stale',q.inputs IS NOT NULL AND q.inputs<>relay_hosted.text_price_inputs(i,q.mode='api_default'),
            'below_cost',EXISTS(SELECT FROM relay_hosted.text_basis_supply(i,false) cost WHERE
                (draft.rates->>'input')::numeric<cost.input_rate OR (draft.rates->>'cached')::numeric<cost.cached_rate
                OR (draft.rates->>'output')::numeric<cost.output_rate)),
        'draft',draft.rates,'effective',CASE WHEN l.id IS NOT NULL THEN relay_hosted.text_rates(o) END)
        ORDER BY i.model,i.source_label,i.id),'[]') FROM relay_hosted.text_inventory i
        JOIN relay_hosted.text_model_rules r ON r.id=i.id JOIN relay_hosted.service_control s ON s.qualification_scope_ref=i.scope_id
        JOIN relay_hosted.installations h ON h.id=s.installation_id AND h.public_origin=origin
        LEFT JOIN relay_hosted.source_serving v ON (v.installation_id,v.source_id)=(h.id,i.source_id)
        LEFT JOIN relay_hosted.current_text_inventory c ON c.id=i.id LEFT JOIN relay_hosted.text_offers o ON o.id=r.last_offer
        LEFT JOIN relay_hosted.active_text_listings l ON l.id=o.id
        LEFT JOIN relay_hosted.text_purchase_rates p ON p.item=i.id AND p.revision=r.purchase_revision
        LEFT JOIN relay_hosted.latest_text_source_markup m ON m.source_id=i.source_id
        LEFT JOIN relay_hosted.latest_text_sale_review q ON q.scope_id=i.scope_id AND q.model=i.model
        CROSS JOIN LATERAL (SELECT coalesce(q.rates,r.draft) AS rates) draft
        CROSS JOIN LATERAL (SELECT i.inventory_epoch=coalesce(v.inventory_epoch,0) AS active) current_binding
        WHERE current_binding.active OR r.enabled);
END $$;
