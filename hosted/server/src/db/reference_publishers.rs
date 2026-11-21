use super::*;
use crate::qualification::valid_model;
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};
use zenith_relay_core::model_metadata::ModelMetadataCatalog;

fn publisher_pairs(source: &str) -> Result<BTreeMap<String, String>, &'static str> {
    let metadata = ModelMetadataCatalog::from_models_dev_json(source)
        .map_err(|_| "invalid Relay reference metadata")?;
    let mut publishers = metadata.reference_publishers();
    publishers.retain(|model, publisher| {
        valid_model(model)
            && (1..=64).contains(&publisher.len())
            && publisher != "other"
            && publisher.as_bytes()[0].is_ascii_alphanumeric()
            && publisher
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
    });
    if !(1..=100_000).contains(&publishers.len()) {
        return Err("reference publisher count is out of range");
    }
    Ok(publishers)
}

/// Local schema-owner operation. Commit only publisher identities, never
/// reference prices, endpoints, capabilities or credentials.
pub async fn import_catalog_publishers(
    pool: &PgPool,
    origin: &str,
    path: &Path,
) -> Result<usize, &'static str> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    const FAILED: &str =
        "cannot import publishers; use the migration role for the unfenced installation";
    if !path
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() <= LIMIT)
    {
        return Err("expected a regular reference metadata file up to 64 MiB");
    }
    let mut source = String::new();
    File::open(path)
        .and_then(|file| file.take(LIMIT + 1).read_to_string(&mut source))
        .map_err(|_| "cannot read reference metadata")?;
    if source.len() as u64 > LIMIT {
        return Err("reference metadata exceeds 64 MiB");
    }
    let publishers = publisher_pairs(&source)?;
    closed_setup(pool, origin).await?;
    let mut tx = pool.begin().await.map_err(|_| FAILED)?;
    let installation: Uuid = sqlx::query_scalar(
        "SELECT h.id FROM relay_hosted.installations h JOIN relay_hosted.service_control s ON s.installation_id=h.id
         WHERE h.public_origin=$1 AND s.restore_operation IS NULL FOR SHARE OF h,s",
    )
    .bind(origin)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| FAILED)?;
    sqlx::query(
        "SELECT 1 FROM relay_hosted.model_display_layout WHERE installation_id=$1 FOR UPDATE",
    )
    .bind(installation)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| FAILED)?;
    let count = publishers.len();
    let (models, names): (Vec<_>, Vec<_>) = publishers.into_iter().unzip();
    let changed = sqlx::query(
        "INSERT INTO relay_hosted.reference_publishers AS p(installation_id,model,publisher)
         SELECT $1,n.model,n.publisher FROM unnest($2::text[],$3::text[]) n(model,publisher)
         ON CONFLICT (installation_id,model) DO UPDATE SET publisher=excluded.publisher
         WHERE p.publisher<>excluded.publisher",
    )
    .bind(installation)
    .bind(&models)
    .bind(&names)
    .execute(&mut *tx)
    .await
    .map_err(|_| FAILED)?
    .rows_affected();
    let removed = sqlx::query(
        "DELETE FROM relay_hosted.reference_publishers p WHERE p.installation_id=$1 AND NOT (p.model=ANY($2))",
    )
    .bind(installation)
    .bind(models)
    .execute(&mut *tx)
    .await
    .map_err(|_| FAILED)?
    .rows_affected();
    if changed + removed > 0 {
        sqlx::query("UPDATE relay_hosted.model_display_layout SET revision=revision+1 WHERE installation_id=$1")
            .bind(installation)
            .execute(&mut *tx)
            .await
            .map_err(|_| FAILED)?;
    }
    tx.commit().await.map_err(|_| FAILED)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_projection_uses_only_registered_identities() {
        let pairs = publisher_pairs(
            r#"{"openai/synthetic":{"name":"Synthetic"},
                "x-ai/gpt-looking":{"name":"Other model"},
                "first/shared":{"name":"First"},
                "second/shared":{"name":"Second"}}"#,
        )
        .unwrap();
        assert_eq!(pairs["synthetic"], "openai");
        assert_eq!(pairs["openai/synthetic"], "openai");
        assert_eq!(pairs["gpt-looking"], "xai");
        assert_eq!(pairs["first/shared"], "first");
        assert_eq!(pairs["second/shared"], "second");
        for unregistered in ["shared", "gateway/synthetic", "gpt-unknown"] {
            assert!(!pairs.contains_key(unregistered));
        }
        assert!(publisher_pairs("[]").is_err());
    }
}
