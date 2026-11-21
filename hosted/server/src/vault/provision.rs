use super::{Error, Purpose, Result, Vault, files, lock};
use sqlx::{Postgres, Transaction};
use std::path::Path;
use subtle::ConstantTimeEq;
use uuid::Uuid;
use zeroize::Zeroizing;

pub enum ImportRoute<T> {
    Direct,
    Proxy(T),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProvisionResult {
    Created,
    PreviouslyCommitted,
}

impl Vault {
    pub async fn provision_files(
        &self,
        operation: Uuid,
        source: Uuid,
        provider: &Path,
        route: ImportRoute<&Path>,
    ) -> Result<ProvisionResult> {
        let key = files::new_slot_input(provider, &self.files.root)?;
        let proxy = match route {
            ImportRoute::Direct => None,
            ImportRoute::Proxy(path) => Some(files::new_slot_input(path, &self.files.root)?),
        };
        let route = match proxy.as_ref() {
            Some(p) => ImportRoute::Proxy(p.as_slice()),
            None => ImportRoute::Direct,
        };
        self.provision_api(operation, source, &key, route).await
    }

    /// Store one static provider credential with its explicitly selected route. No network or admission.
    pub async fn provision_api(
        &self,
        operation: Uuid,
        source: Uuid,
        provider: &[u8],
        route: ImportRoute<&[u8]>,
    ) -> Result<ProvisionResult> {
        let proxy = match route {
            ImportRoute::Direct => None,
            ImportRoute::Proxy(bytes) => {
                let value = std::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
                if value.trim() != value {
                    return Err(Error::Invalid);
                }
                Some(Zeroizing::new(
                    zenith_relay_core::normalize_proxy_url(value).map_err(|_| Error::Invalid)?,
                ))
            }
        };
        let proxy = proxy.as_ref().map(|p| p.as_bytes());
        let mut tx = lock(&self.pool, true).await?;
        let result = self
            .stage_api(&mut tx, operation, source, provider, proxy)
            .await?;
        if result == ProvisionResult::Created {
            tx.commit().await.map_err(|_| Error::Database)?;
        }
        Ok(result)
    }

    /// Caller owns the exclusive vault lock and the transaction, including its receipt.
    pub(super) async fn stage_api(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        operation: Uuid,
        source: Uuid,
        provider: &[u8],
        proxy: Option<&[u8]>,
    ) -> Result<ProvisionResult> {
        if operation.is_nil()
            || source.is_nil()
            || !(1..=1024).contains(&provider.len())
            || !provider.iter().all(u8::is_ascii_graphic)
        {
            return Err(Error::Invalid);
        }
        self.files.bind_marker(Some(&self.marker))?;
        let tag = self
            .files
            .provision_tag(operation, source, provider, proxy)?;
        let prior = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT payload_tag FROM relay_hosted.static_source_provisions
             WHERE installation_id=$1 AND operation_id=$2",
        )
        .bind(self.installation)
        .bind(operation)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| Error::Database)?;
        if let Some(prior) = prior {
            return if bool::from(tag.as_ref().ct_eq(&prior)) {
                Ok(ProvisionResult::PreviouslyCommitted)
            } else {
                Err(Error::Conflict)
            };
        }
        for purpose in [Purpose::Provider, Purpose::Proxy] {
            // A direct import must not adopt an unrelated existing proxy or a revoked slot.
            if self.slot(tx, source, purpose).await?.is_some() {
                return Err(Error::Conflict);
            }
        }
        self.put_slot(tx, source, Purpose::Provider, 0, provider)
            .await?;
        if let Some(bytes) = proxy {
            self.put_slot(tx, source, Purpose::Proxy, 0, bytes).await?;
        }
        sqlx::query(
            "INSERT INTO relay_hosted.static_source_provisions
             (installation_id,operation_id,source_id,proxied,payload_tag) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(self.installation)
        .bind(operation)
        .bind(source)
        .bind(proxy.is_some())
        .bind(tag.as_ref())
        .execute(&mut **tx)
        .await
        .map_err(|_| Error::Database)?;
        Ok(ProvisionResult::Created)
    }
}
