use super::{Error, ProvisionResult, Result, Vault, lock_transaction};
use ring::{digest, hmac};
use uuid::Uuid;
use zeroize::Zeroizing;

impl Vault {
    /// A one-use, session-bound permit is consumed only with the encrypted slots.
    pub(crate) async fn commit_web_source(
        &self,
        origin: &str,
        op: Uuid,
        verifier: &[u8; 32],
        body: &[u8],
    ) -> Result<Option<Uuid>> {
        crate::db::closed_setup(&self.pool, origin)
            .await
            .map_err(|_| Error::Database)?;
        let digest = digest::digest(&digest::SHA256, verifier);
        let mut tx = self.pool.begin().await.map_err(|_| Error::Database)?;
        let grant: Option<(Uuid, bool, Vec<u8>)> =
            sqlx::query_as("SELECT * FROM relay_hosted.web_source_access($1,$2,$3,$4)")
                .bind(origin)
                .bind(self.installation)
                .bind(op)
                .bind(digest.as_ref())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| Error::Database)?;
        let Some((source, proxied, mac)) = grant else {
            return Ok(None);
        };
        hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, verifier), body, &mac)
            .map_err(|_| Error::Conflict)?;
        let (provider, raw_proxy) = std::str::from_utf8(body)
            .ok()
            .and_then(|v| v.split_once('\n'))
            .ok_or(Error::Invalid)?;
        if proxied == raw_proxy.is_empty() || raw_proxy.trim() != raw_proxy {
            return Err(Error::Invalid);
        }
        let proxy = if proxied {
            let normalized =
                zenith_relay_core::normalize_proxy_url(raw_proxy).map_err(|_| Error::Invalid)?;
            Some(Zeroizing::new(normalized))
        } else {
            None
        };
        lock_transaction(&mut tx, true).await?;
        if self
            .stage_api(
                &mut tx,
                op,
                source,
                provider.as_bytes(),
                proxy.as_ref().map(|v| v.as_bytes()),
            )
            .await?
            != ProvisionResult::Created
        {
            return Err(Error::Conflict);
        }
        let finished: bool =
            sqlx::query_scalar("SELECT relay_hosted.finish_web_source($1,$2,$3,$4)")
                .bind(origin)
                .bind(self.installation)
                .bind(op)
                .bind(digest.as_ref())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| Error::Database)?;
        if !finished {
            return Ok(None);
        }
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(Some(source))
    }
}
