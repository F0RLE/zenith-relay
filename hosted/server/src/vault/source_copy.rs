use super::{Error, ProvisionResult, Result, Vault, lock_transaction};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
use zenith_relay_hosted_device::{CopyPlan, CopyProof, CopyState};
use zeroize::Zeroizing;

type Grant = (String, Option<sqlx::types::Json<CopyPlan>>, Option<Vec<u8>>);

fn state_of(row: &Grant) -> Result<CopyState> {
    match row.0.as_str() {
        "awaiting_approval" => Ok(CopyState::AwaitingApproval),
        "approved" => Ok(CopyState::Approved),
        "expired" => Ok(CopyState::Expired),
        "committed" => Ok(CopyState::Committed),
        _ => Err(Error::Database),
    }
}

async fn grant(
    tx: &mut Transaction<'_, Postgres>,
    origin: &str,
    proof: &CopyProof,
    applying: bool,
) -> Result<Option<Grant>> {
    sqlx::query_as("SELECT * FROM relay_hosted.source_copy_access($1,$2,$3,$4,$5,$6)")
        .bind(origin)
        .bind(proof.installation_id)
        .bind(proof.device_id)
        .bind(proof.operation_id)
        .bind(proof.verifier().as_slice())
        .bind(applying)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| Error::Database)
}

impl Vault {
    pub(crate) async fn check_copy_worker(
        &self,
        origin: &str,
    ) -> std::result::Result<(), &'static str> {
        if !origin.starts_with("https://") {
            return Err("source copies require an HTTPS public origin");
        }
        crate::db::closed_setup(&self.pool, origin).await?;
        let ok: bool = sqlx::query_scalar("SELECT
            has_function_privilege('relay_hosted.source_copy_access(text,uuid,uuid,uuid,bytea,boolean)','EXECUTE')
            AND has_function_privilege('relay_hosted.finish_source_copy(text,uuid,uuid,uuid,bytea)','EXECUTE')
            AND has_function_privilege('relay_hosted.web_source_access(text,uuid,uuid,bytea)','EXECUTE')
            AND has_function_privilege('relay_hosted.finish_web_source(text,uuid,uuid,bytea)','EXECUTE')
            AND NOT has_function_privilege('relay_hosted.approve_web_source(text,bytea,uuid,text,text,text[],boolean,bytea,bytea,bytea,bigint,bigint,boolean,boolean,bigint)','EXECUTE')
            AND NOT has_function_privilege('relay_hosted.web_source_receipt(text,bytea,uuid)','EXECUTE')
            AND NOT has_any_column_privilege('relay_hosted.web_source_intakes','SELECT,INSERT,UPDATE')
            AND NOT has_table_privilege('relay_hosted.web_source_intakes','DELETE,TRUNCATE')
            AND NOT has_function_privilege('relay_hosted.review_source_copy(text,bytea,uuid,uuid,uuid,uuid,jsonb,bytea,bytea,bigint,bytea,bigint,bigint,boolean,boolean,bigint)','EXECUTE')
            AND NOT has_any_column_privilege('relay_hosted.desktop_source_copies','SELECT,INSERT,UPDATE')
            AND NOT has_table_privilege('relay_hosted.desktop_source_copies','DELETE,TRUNCATE')")
            .fetch_one(&self.pool).await.map_err(|_| "cannot verify source copy role")?;
        if ok {
            Ok(())
        } else {
            Err("source copies require scoped vault grants")
        }
    }

    pub(crate) async fn copy_status(
        &self,
        origin: &str,
        proof: &CopyProof,
    ) -> Result<Option<CopyState>> {
        if proof.installation_id != self.installation {
            return Ok(None);
        }
        crate::db::closed_setup(&self.pool, origin)
            .await
            .map_err(|_| Error::Database)?;
        let mut tx = self.pool.begin().await.map_err(|_| Error::Database)?;
        grant(&mut tx, origin, proof, false)
            .await?
            .as_ref()
            .map(state_of)
            .transpose()
    }

    pub(crate) async fn commit_source_copy(
        &self,
        origin: &str,
        proof: &CopyProof,
        provider: &str,
        proxy: Option<&str>,
    ) -> Result<Option<CopyState>> {
        if proof.installation_id != self.installation {
            return Ok(None);
        }
        let proxy = proxy
            .map(|raw| {
                let normalized = Zeroizing::new(
                    zenith_relay_core::normalize_proxy_url(raw).map_err(|_| Error::Invalid)?,
                );
                (normalized.as_str() == raw)
                    .then_some(normalized)
                    .ok_or(Error::Invalid)
            })
            .transpose()?;
        let proxy = proxy.as_ref().map(|p| p.as_str());
        let mut tx = self.pool.begin().await.map_err(|_| Error::Database)?;
        let Some(row) = grant(&mut tx, origin, proof, true).await? else {
            return Ok(None);
        };
        let state = state_of(&row)?;
        if state != CopyState::Approved {
            return Ok(Some(state));
        }
        let (_, Some(plan), Some(tag)) = row else {
            return Err(Error::Database);
        };
        if !plan.valid()
            || plan.origin != origin
            || !proof.matches_payload(&plan, &tag, provider, proxy)
        {
            return Err(Error::Conflict);
        }
        // Like qualification, take the authority fence before the vault lock.
        lock_transaction(&mut tx, true).await?;
        let current = grant(&mut tx, origin, proof, false)
            .await?
            .as_ref()
            .map(state_of)
            .transpose()?;
        if current != Some(CopyState::Approved) {
            return Ok(current);
        }
        let result = self
            .stage_api(
                &mut tx,
                proof.operation_id,
                plan.source_id,
                provider.as_bytes(),
                proxy.map(str::as_bytes),
            )
            .await?;
        if result != ProvisionResult::Created {
            return Err(Error::Conflict);
        }
        let finished: bool =
            sqlx::query_scalar("SELECT relay_hosted.finish_source_copy($1,$2,$3,$4,$5)")
                .bind(origin)
                .bind(proof.installation_id)
                .bind(proof.device_id)
                .bind(proof.operation_id)
                .bind(proof.verifier().as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| Error::Database)?;
        if !finished {
            return Ok(Some(CopyState::Expired));
        }
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(Some(CopyState::Committed))
    }

    pub(crate) async fn check_saved_route(
        &self,
        source: Uuid,
        endpoint: &str,
        proxied: bool,
    ) -> Result<()> {
        let matches: bool =
            sqlx::query_scalar("SELECT relay_hosted.source_copy_route($1,$2,$3,$4)")
                .bind(self.installation)
                .bind(source)
                .bind(endpoint)
                .bind(proxied)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| Error::Database)?;
        if matches {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }
}
