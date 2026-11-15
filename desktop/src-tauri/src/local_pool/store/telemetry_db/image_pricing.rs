use super::*;
use zenith_relay_core::usage::{ImagePriceComponent, image_price_components};
use zenith_relay_core::{ApiEquivalentUsage, CatalogPriceResolver, pricing::PriceSource};

fn json_error(_: serde_json::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "invalid image pricing component")
}

pub(super) fn apply_delta(
    transaction: &rusqlite::Transaction<'_>,
    kind: &str,
    member: &str,
    model: &str,
    parts: &[ImagePriceComponent],
    multiplier: i64,
) -> Result<()> {
    for part in parts {
        let component = serde_json::to_string(&part.kind).map_err(json_error)?;
        transaction.execute(
            "INSERT INTO usage_image_price_rollups(candidate_kind, candidate_id, model, component,
             price_model, price_class, context_band, tokens, images)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(candidate_kind,candidate_id,model,component,price_model,price_class,context_band)
             DO UPDATE SET tokens=tokens+excluded.tokens, images=images+excluded.images",
            params![kind, member, model, component, part.model, part.price_class, part.context_band,
                sql_u64(part.tokens) * multiplier, sql_u64(part.images) * multiplier],
        ).map_err(db_error)?;
    }
    Ok(())
}

pub(super) struct ImageEstimate {
    pub kind: String,
    pub member: String,
    pub model: String,
    pub at_ms: u64,
    pub value: ApiEquivalentSummary,
    pub source: PriceSource,
}

/// Scalar rollups retain the complete token denominator; these priced subsets
/// replace unpriced coverage, rather than adding a second copy of those tokens.
pub(super) fn add_priced(target: &mut ApiEquivalentSummary, value: ApiEquivalentSummary) {
    target.micro_usd = target.micro_usd.saturating_add(value.micro_usd);
    target.priced_tokens = target.priced_tokens.saturating_add(value.priced_tokens);
    target.unpriced_tokens = target.unpriced_tokens.saturating_sub(value.priced_tokens);
}

pub(super) fn estimates(
    connection: &Connection,
    where_sql: &str,
    parameters: &[SqlValue],
    resolver: &CatalogPriceResolver<'_>,
    rollup: bool,
    bucket: Option<(u64, u64)>,
) -> Result<Vec<ImageEstimate>> {
    let mut result = Vec::new();
    if rollup && where_sql.is_empty() {
        let mut statement = connection.prepare(
            "SELECT candidate_kind,candidate_id,model,component,price_model,price_class,context_band,tokens,images
             FROM usage_image_price_rollups WHERE tokens>0 OR images>0"
        ).map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let kind: String = row.get(0).map_err(db_error)?;
            let member: String = row.get(1).map_err(db_error)?;
            let part = ImagePriceComponent {
                kind: serde_json::from_str(&row.get::<_, String>(3).map_err(db_error)?)
                    .map_err(json_error)?,
                model: row.get(4).map_err(db_error)?,
                price_class: row.get(5).map_err(db_error)?,
                context_band: row.get(6).map_err(db_error)?,
                tokens: row.get::<_, i64>(7).map_err(db_error)?.max(0) as u64,
                images: row.get::<_, i64>(8).map_err(db_error)?.max(0) as u64,
            };
            let (value, source) = resolver.estimate_image_component(&kind, &member, &part);
            result.push(ImageEstimate {
                kind,
                member,
                model: row.get(2).map_err(db_error)?,
                at_ms: 0,
                value,
                source,
            });
        }
        return Ok(result);
    }
    let conjunction = if where_sql.is_empty() {
        " WHERE "
    } else {
        " AND "
    };
    let filter = format!("{where_sql}{conjunction}image_usage_json IS NOT NULL");
    let projection = super::usage::send_projection(
        "created_at,account_id,source_id,resolved_model,requested_model,input_tokens,output_tokens,total_tokens,applied_service_tier,image_usage_json",
        &filter,
    );
    let mut statement = connection
        .prepare(&format!(
            "SELECT CASE WHEN account_id IS NULL THEN 'source' ELSE 'account' END,
         COALESCE(account_id,source_id),COALESCE(resolved_model,requested_model,''),
         input_tokens,output_tokens,total_tokens,applied_service_tier,image_usage_json,
         CAST(strftime('%s',created_at) AS INTEGER)*1000 FROM ({projection})"
        ))
        .map_err(db_error)?;
    let mut rows = statement
        .query(params_from_iter(parameters.iter().chain(parameters.iter())))
        .map_err(db_error)?;
    type ComponentKey = (String, String, String, u64, String);
    let mut components = std::collections::BTreeMap::<ComponentKey, ImagePriceComponent>::new();
    while let Some(row) = rows.next().map_err(db_error)? {
        let kind: String = row.get(0).map_err(db_error)?;
        let member: String = row.get(1).map_err(db_error)?;
        let model: String = row.get(2).map_err(db_error)?;
        let image_json: String = row.get(7).map_err(db_error)?;
        let Ok(image) = serde_json::from_str(&image_json) else {
            continue;
        };
        let tokens = |index| {
            row.get::<_, Option<i64>>(index)
                .map(|value| value.map(|value| value.max(0) as u64))
                .map_err(db_error)
        };
        let usage = ApiEquivalentUsage {
            input_tokens: tokens(3)?,
            output_tokens: tokens(4)?,
            total_tokens: tokens(5)?,
            ..Default::default()
        }
        .with_observed_rates(
            row.get::<_, Option<String>>(6)
                .map_err(db_error)?
                .as_deref(),
            tokens(3)?,
        );
        let at_ms = tokens(8)?.unwrap_or_default();
        let at_ms = bucket.map_or(0, |(start, width)| {
            start + at_ms.saturating_sub(start) / width * width
        });
        for mut part in image_price_components(&model, usage, &image) {
            let (tokens, images) = (part.tokens, part.images);
            part.tokens = 0;
            part.images = 0;
            let key = (
                kind.clone(),
                member.clone(),
                model.clone(),
                at_ms,
                serde_json::to_string(&part).map_err(json_error)?,
            );
            let sum = components.entry(key).or_insert(part);
            sum.tokens = sum.tokens.saturating_add(tokens);
            sum.images = sum.images.saturating_add(images);
        }
    }
    for ((kind, member, model, at_ms, _), part) in components {
        let (value, source) = resolver.estimate_image_component(&kind, &member, &part);
        result.push(ImageEstimate {
            kind,
            member,
            model,
            at_ms,
            value,
            source,
        });
    }
    Ok(result)
}
