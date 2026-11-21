//! Private connection storage. Only the transfer worker accepts browser source secrets.
mod files;
mod provision;
mod source_copy;
mod source_intake;
pub(crate) use files::read_operator_key;
pub use provision::{ImportRoute, ProvisionResult};

use files::Files;
use sqlx::{PgPool, Postgres, Transaction};
use std::path::Path;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug)]
pub enum Purpose {
    Provider = 1,
    Proxy = 2,
}

impl Purpose {
    fn code(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Proxy => "proxy",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Database,
    Bind,
    Role,
    Storage,
    Stale,
    Invalid,
    Conflict,
}

impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Self::Database => "vault database unavailable",
            Self::Bind => "vault binding could not be committed",
            Self::Role => "a separate vault database role is required",
            Self::Storage => "vault key or protected storage unavailable",
            Self::Stale => "secret revision is no longer current",
            Self::Invalid => "invalid secret or revision",
            Self::Conflict => "source or operation already belongs to a different import",
        }
    }
}

type Result<T> = std::result::Result<T, Error>;
type Slot = (i64, Option<Uuid>, Option<Vec<u8>>);

#[derive(Clone)]
pub struct Vault {
    files: Files,
    pool: PgPool,
    installation: Uuid,
    marker: [u8; 32],
}

async fn lock(pool: &PgPool, exclusive: bool) -> Result<Transaction<'_, Postgres>> {
    let mut tx = pool.begin().await.map_err(|_| Error::Database)?;
    lock_transaction(&mut tx, exclusive).await?;
    Ok(tx)
}

async fn lock_transaction(tx: &mut Transaction<'_, Postgres>, exclusive: bool) -> Result<()> {
    // One advisory namespace for bind, staging/commit, readers and orphan collection.
    let query = if exclusive {
        "SELECT pg_advisory_xact_lock(98653249, 1)"
    } else {
        "SELECT pg_advisory_xact_lock_shared(98653249, 1)"
    };
    sqlx::query(query)
        .execute(&mut **tx)
        .await
        .map_err(|_| Error::Database)?;
    Ok(())
}

impl Vault {
    pub async fn open(pool: &PgPool, origin: &str, root: &Path, key: &Path) -> Result<Self> {
        crate::db::closed_setup(pool, origin)
            .await
            .map_err(|_| Error::Database)?;
        let mut tx = lock(pool, true).await?;
        let privileged: bool = sqlx::query_scalar(
            "SELECT has_schema_privilege('relay_hosted','CREATE') OR
                has_database_privilege(current_database(),'CREATE') OR
                has_function_privilege('relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)','EXECUTE') OR
                has_function_privilege('relay_hosted.owner_login_context(text)','EXECUTE') OR
                has_function_privilege('relay_hosted.post_verified_payment(uuid,text,text,text,bigint)','EXECUTE') OR
                has_any_column_privilege('relay_hosted.admin_credentials','SELECT') OR
                has_table_privilege('relay_hosted.connection_secret_slots','DELETE, TRUNCATE') OR
                has_table_privilege('relay_hosted.vault_bindings','DELETE, TRUNCATE') OR
                has_any_column_privilege('relay_hosted.vault_bindings','UPDATE') OR
                has_table_privilege('relay_hosted.static_source_provisions','UPDATE, DELETE, TRUNCATE')",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| Error::Role)?;
        if privileged {
            return Err(Error::Role);
        }
        let installation = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM relay_hosted.installations WHERE singleton AND public_origin=$1",
        )
        .bind(origin)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| Error::Database)?;
        let known = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT marker_digest FROM relay_hosted.vault_bindings WHERE installation_id=$1 AND key_version=1",
        ).bind(installation).fetch_optional(&mut *tx).await.map_err(|_| Error::Database)?;
        let files = Files::open(root, key, installation)?;
        let digest = files.bind_marker(known.as_deref())?;
        if known.is_none() {
            sqlx::query("INSERT INTO relay_hosted.vault_bindings (installation_id,key_version,marker_digest) VALUES ($1,1,$2)")
                .bind(installation).bind(digest.as_slice()).execute(&mut *tx).await.map_err(|_| Error::Bind)?;
        }
        tx.commit().await.map_err(|_| Error::Bind)?;
        Ok(Self {
            files,
            pool: pool.clone(),
            installation,
            marker: digest,
        })
    }

    async fn slot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        purpose: Purpose,
    ) -> Result<Option<Slot>> {
        sqlx::query_as(
            "SELECT revision,generation,manifest_digest FROM relay_hosted.connection_secret_slots
            WHERE installation_id=$1 AND connection_id=$2 AND purpose=$3",
        )
        .bind(self.installation)
        .bind(id)
        .bind(purpose.code())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| Error::Database)
    }

    pub async fn add_from_file(&self, id: Uuid, purpose: Purpose, file: &Path) -> Result<i64> {
        let input = files::new_slot_input(file, &self.files.root)?;
        self.put(id, purpose, 0, &input).await
    }

    /// Expect zero only for a new slot. A revoked slot cannot be resurrected.
    pub async fn put(&self, id: Uuid, purpose: Purpose, expected: i64, data: &[u8]) -> Result<i64> {
        if expected < 0 || id.is_nil() {
            return Err(Error::Invalid);
        }
        let mut tx = lock(&self.pool, true).await?;
        self.files.bind_marker(Some(&self.marker))?;
        let next = self.put_slot(&mut tx, id, purpose, expected, data).await?;
        // An ambiguous commit must not delete either candidate generation.
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(next)
    }

    async fn put_slot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: Uuid,
        purpose: Purpose,
        expected: i64,
        data: &[u8],
    ) -> Result<i64> {
        let next = expected.checked_add(1).ok_or(Error::Invalid)?;
        match self.slot(tx, id, purpose).await? {
            None if expected == 0 => (),
            Some((r, Some(_), Some(_))) if expected == r => (),
            _ => return Err(Error::Stale),
        }
        let generation = Uuid::new_v4();
        let digest = self.files.seal(id, purpose, next, generation, data)?;
        let changed = sqlx::query("INSERT INTO relay_hosted.connection_secret_slots
            (installation_id,connection_id,purpose,revision,generation,manifest_digest) VALUES ($1,$2,$3,$4,$5,$6)
            ON CONFLICT (installation_id,connection_id,purpose) DO UPDATE SET revision=EXCLUDED.revision,
                generation=EXCLUDED.generation,manifest_digest=EXCLUDED.manifest_digest
            WHERE connection_secret_slots.revision=$7 AND connection_secret_slots.generation IS NOT NULL")
            .bind(self.installation).bind(id).bind(purpose.code()).bind(next).bind(generation)
            .bind(digest.as_slice()).bind(expected).execute(&mut **tx).await.map_err(|_| Error::Database)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Stale);
        }
        Ok(next)
    }

    pub async fn read(
        &self,
        id: Uuid,
        purpose: Purpose,
        expected: i64,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let mut tx = lock(&self.pool, false).await?;
        self.files.bind_marker(Some(&self.marker))?;
        let Some((revision, Some(generation), Some(digest))) =
            self.slot(&mut tx, id, purpose).await?
        else {
            return Err(Error::Stale);
        };
        if revision != expected {
            return Err(Error::Stale);
        }
        let data = self
            .files
            .unseal(id, purpose, revision, generation, &digest)?;
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(data)
    }

    pub async fn revoke(&self, id: Uuid, purpose: Purpose, expected: i64) -> Result<i64> {
        let next = expected.checked_add(1).ok_or(Error::Invalid)?;
        let mut tx = lock(&self.pool, true).await?;
        let changed = sqlx::query(
            "UPDATE relay_hosted.connection_secret_slots SET generation=NULL,
            manifest_digest=NULL, revision=$4 WHERE installation_id=$1 AND connection_id=$2
            AND purpose=$3 AND revision=$5 AND generation IS NOT NULL",
        )
        .bind(self.installation)
        .bind(id)
        .bind(purpose.code())
        .bind(next)
        .bind(expected)
        .execute(&mut *tx)
        .await
        .map_err(|_| Error::Database)?;
        if changed.rows_affected() != 1 {
            return Err(Error::Stale);
        }
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(next)
    }

    /// Run only for an isolated installation with no archived-generation retention.
    pub async fn collect_unreferenced(&self) -> Result<usize> {
        let mut tx = lock(&self.pool, true).await?;
        self.files.bind_marker(Some(&self.marker))?;
        let live = sqlx::query_scalar::<_, Uuid>(
            "SELECT generation FROM relay_hosted.connection_secret_slots
            WHERE installation_id=$1 AND generation IS NOT NULL LIMIT 16385",
        )
        .bind(self.installation)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| Error::Database)?;
        if live.len() > 16384 {
            return Err(Error::Storage);
        }
        let live: std::collections::HashSet<_> = live.into_iter().collect();
        let mut removed = 0;
        for id in self.files.candidates()? {
            if !live.contains(&id) {
                std::fs::remove_file(self.files.generation(id)).map_err(|_| Error::Storage)?;
                removed += 1;
            }
        }
        tx.commit().await.map_err(|_| Error::Database)?;
        Ok(removed)
    }
}
