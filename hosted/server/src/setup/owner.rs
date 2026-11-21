use super::{LIFETIME, Result};
use sqlx::{PgConnection, PgPool, types::Json};
use webauthn_rs::prelude::{Passkey, Uuid};

#[derive(Clone, Copy)]
pub enum OwnerOperation {
    Setup,
    Recover { installation: Uuid, operation: Uuid },
}

impl OwnerOperation {
    pub(super) fn page(self) -> &'static str {
        match self {
            Self::Setup => "/setup/owner",
            Self::Recover { .. } => "/setup/recovery",
        }
    }
}

pub(super) struct Target {
    installation: Uuid,
    installation_epoch: i64,
    pub(super) user: Uuid,
    deadline_ms: i64,
    recovery: Option<Recovery>,
}

struct Recovery {
    operation: Uuid,
    owner_epoch: i64,
    service_revision: i64,
}

type RecoveryContext = (i64, Uuid, i64, i64, i64, bool);

impl Target {
    // None means this recovery operation was already committed.
    pub(super) async fn load(
        pool: &PgPool,
        origin: &str,
        action: OwnerOperation,
    ) -> Result<Option<Self>> {
        match action {
            OwnerOperation::Setup => {
                let (installation, installation_epoch, deadline_ms) = sqlx::query_as::<_, (Uuid, i64, i64)>(
                    "SELECT id, authorization_epoch, floor(extract(epoch FROM clock_timestamp())*1000)::bigint + $2
                     FROM relay_hosted.pending_owner WHERE public_origin=$1
                     AND has_function_privilege('relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)','EXECUTE')
                     AND NOT has_function_privilege('relay_hosted.post_verified_payment(uuid,text,text,text,bigint)','EXECUTE')
                     AND NOT has_schema_privilege('relay_hosted','CREATE')",
                )
                .bind(origin)
                .bind(LIFETIME.as_millis() as i64)
                .fetch_optional(pool)
                .await
                .map_err(|_| "local setup role required")?
                .ok_or("owner already provisioned or setup role unavailable")?;
                Ok(Some(Self {
                    installation,
                    installation_epoch,
                    user: Uuid::new_v4(),
                    deadline_ms,
                    recovery: None,
                }))
            }
            OwnerOperation::Recover {
                installation,
                operation,
            } => {
                if installation.is_nil() || operation.is_nil() {
                    return Err("owner recovery requires non-nil installation and operation IDs");
                }
                let (installation_epoch, user, owner_epoch, service_revision, deadline_ms, committed) =
                    sqlx::query_as::<_, RecoveryContext>(
                        "SELECT * FROM relay_hosted.owner_recovery_context($1,$2,$3)
                         WHERE has_function_privilege('relay_hosted.replace_owner_passkey(uuid,text,uuid,bigint,uuid,bigint,bigint,bytea,jsonb,bigint)','EXECUTE')
                         AND NOT has_function_privilege('relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)','EXECUTE')
                         AND NOT has_function_privilege('relay_hosted.read_admin_overview(text,bytea)','EXECUTE')
                         AND NOT has_function_privilege('relay_hosted.post_verified_payment(uuid,text,text,text,bigint)','EXECUTE')
                         AND NOT has_schema_privilege('relay_hosted','CREATE')",
                    )
                    .bind(installation)
                    .bind(origin)
                    .bind(operation)
                    .fetch_optional(pool)
                    .await
                    .map_err(|_| "local owner recovery role required")?
                    .ok_or("owner recovery unavailable for this installation or database role")?;
                Ok((!committed).then_some(Self {
                    installation,
                    installation_epoch,
                    user,
                    deadline_ms,
                    recovery: Some(Recovery {
                        operation,
                        owner_epoch,
                        service_revision,
                    }),
                }))
            }
        }
    }

    pub(super) async fn store(
        &self,
        tx: &mut PgConnection,
        origin: &str,
        key: &Passkey,
    ) -> std::result::Result<bool, sqlx::Error> {
        if let Some(recovery) = &self.recovery {
            sqlx::query_scalar(
                "SELECT relay_hosted.replace_owner_passkey($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(self.installation)
            .bind(origin)
            .bind(recovery.operation)
            .bind(self.installation_epoch)
            .bind(self.user)
            .bind(recovery.owner_epoch)
            .bind(recovery.service_revision)
            .bind(key.cred_id().as_slice())
            .bind(Json(key))
            .bind(self.deadline_ms)
            .fetch_one(tx)
            .await
        } else {
            sqlx::query_scalar("SELECT relay_hosted.bind_first_owner($1,$2,$3,$4,$5,$6,$7)")
                .bind(self.installation)
                .bind(self.installation_epoch)
                .bind(origin)
                .bind(self.user)
                .bind(key.cred_id().as_slice())
                .bind(Json(key))
                .bind(self.deadline_ms)
                .fetch_one(tx)
                .await
        }
    }
}
