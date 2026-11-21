//! Transaction ordering for account removal. The host holds mutation/credential
//! guards and dispatch fences until this service and runtime retirement finish.
use std::future::Future;

pub trait RemovalHost: Sync {
    type Prepared: Send + Sync;
    type Error: Send + Sync;
    /// On error undo this item's partial side effects before returning.
    fn prepare(
        &self,
        account_id: &str,
    ) -> impl Future<Output = Result<Self::Prepared, Self::Error>> + Send;
    fn commit(&self) -> Result<(), Self::Error>;
    /// Restore prepared items in reverse order. If provider revocation or
    /// another irreversible step prevents recovery, keep dispatch fail-closed.
    fn rollback(
        &self,
        prepared: &[Self::Prepared],
        cause: &Self::Error,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn retire(&self) -> impl Future<Output = ()> + Send;
}

pub async fn remove_accounts<H: RemovalHost>(
    host: &H,
    account_ids: &[String],
) -> Result<(), H::Error> {
    let mut prepared = Vec::with_capacity(account_ids.len());
    for account_id in account_ids {
        match host.prepare(account_id).await {
            Ok(item) => prepared.push(item),
            Err(error) => {
                host.rollback(&prepared, &error).await?;
                return Err(error);
            }
        }
    }
    if let Err(error) = host.commit() {
        host.rollback(&prepared, &error).await?;
        return Err(error);
    }
    host.retire().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Host {
        trace: Mutex<Vec<String>>,
        fail_prepare: bool,
        fail_commit: bool,
        fail_rollback: bool,
    }
    impl RemovalHost for Host {
        type Prepared = String;
        type Error = &'static str;
        async fn prepare(&self, id: &str) -> Result<String, Self::Error> {
            if self.fail_prepare && id == "c" {
                return Err("prepare");
            }
            self.trace.lock().unwrap().push(format!("prepare:{id}"));
            Ok(id.into())
        }
        fn commit(&self) -> Result<(), Self::Error> {
            self.trace.lock().unwrap().push("commit".into());
            if self.fail_commit {
                Err("commit")
            } else {
                Ok(())
            }
        }
        async fn rollback(&self, prepared: &[String], _: &Self::Error) -> Result<(), Self::Error> {
            self.trace
                .lock()
                .unwrap()
                .extend(prepared.iter().rev().map(|id| format!("restore:{id}")));
            if self.fail_rollback {
                Err("recovery")
            } else {
                Ok(())
            }
        }
        async fn retire(&self) {
            self.trace.lock().unwrap().push("retire".into());
        }
    }

    #[tokio::test]
    async fn failed_preparation_restores_previous_items_without_committing_or_retiring() {
        let host = Host {
            trace: Mutex::default(),
            fail_prepare: true,
            fail_commit: false,
            fail_rollback: false,
        };
        assert_eq!(
            remove_accounts(&host, &["a".into(), "b".into(), "c".into()]).await,
            Err("prepare")
        );
        assert_eq!(
            *host.trace.lock().unwrap(),
            ["prepare:a", "prepare:b", "restore:b", "restore:a"]
        );
    }

    #[tokio::test]
    async fn commit_failure_surfaces_incomplete_recovery_without_retiring() {
        let host = Host {
            trace: Mutex::default(),
            fail_prepare: false,
            fail_commit: true,
            fail_rollback: true,
        };
        assert_eq!(remove_accounts(&host, &["a".into()]).await, Err("recovery"));
        assert_eq!(
            *host.trace.lock().unwrap(),
            ["prepare:a", "commit", "restore:a"]
        );
    }

    #[tokio::test]
    async fn retirement_follows_durable_commit() {
        let host = Host {
            trace: Mutex::default(),
            fail_prepare: false,
            fail_commit: false,
            fail_rollback: false,
        };
        remove_accounts(&host, &["a".into()]).await.unwrap();
        assert_eq!(
            *host.trace.lock().unwrap(),
            ["prepare:a", "commit", "retire"]
        );
    }
}
