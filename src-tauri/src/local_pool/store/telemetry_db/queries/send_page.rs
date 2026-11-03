use super::super::*;

impl TelemetryDb {
    /// Read only the selected sends and one lookahead key. No history count,
    /// summary, model/member aggregate, bucket or speculative next-page read.
    pub fn usage_send_page_with_pricing(
        &self,
        query: &UsageQuery,
        catalog: &PricingCatalog,
        context: &PricingContext,
    ) -> Result<LocalUsagePage> {
        let resolver = CatalogPriceResolver::new(catalog, context);
        let (page, page_size) = query.normalized_page();
        let offset = u64::from(page.saturating_sub(1)) * u64::from(page_size);
        let connection = self.lock_report_connection()?;
        let (where_sql, parameters) = usage_filter(query);
        let filter_options = query
            .include_filter_options
            .then(|| self.usage_filter_options_at_revision(&connection, query, connection.revision))
            .transpose()?;
        let mut has_more = false;
        let mut events = if query.includes_events() {
            super::page::usage_send_page(
                &connection,
                &where_sql,
                &parameters,
                page_size,
                offset,
                Some(&mut has_more),
            )?
        } else {
            Vec::new()
        };
        drop(connection);
        super::page::price_events(&mut events, &resolver);
        let known_total = if events.is_empty() {
            0
        } else {
            offset + events.len() as u64 + u64::from(has_more)
        };
        Ok(LocalUsagePage {
            events,
            has_more: Some(has_more),
            workload: query.workload,
            filter_options,
            total: known_total,
            page,
            page_size,
            total_pages: UsageQuery::page_count(known_total, page_size),
            totals: UsageTotals::default(),
            models: Vec::new(),
            pool_members: Vec::new(),
            buckets: Vec::new(),
            pricing: resolver.pricing_metadata(ApiEquivalentSummary::default(), &[]),
        })
    }
}
