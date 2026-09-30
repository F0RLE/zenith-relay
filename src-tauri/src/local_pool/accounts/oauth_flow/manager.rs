use super::*;

impl<B, E> OAuthFlowManager<B, E>
where
    B: SecretBackend + Send + Sync + 'static,
    E: OAuthFlowEventSink,
{
    pub fn new(root: PathBuf, secrets: B, events: E) -> Self {
        Self {
            inner: Arc::new(OAuthFlowInner {
                root,
                secrets,
                events,
                listeners: Mutex::new(HashMap::new()),
                mutation: Mutex::new(()),
            }),
        }
    }

    pub async fn start_for_account(
        &self,
        oauth: &CodexOAuthClient,
        target_account_id: Option<&str>,
        sign_in_proxy_id: Option<&str>,
    ) -> Result<OAuthFlowStart, OAuthFlowError> {
        let now_ms = now_ms();
        for mut snapshot in load_snapshots(&self.inner.root)? {
            if snapshot.target_account_id.as_deref() != target_account_id {
                continue;
            }
            if snapshot.pending.expires_at_ms() <= now_ms {
                self.inner.cleanup(&snapshot.login_id)?;
                continue;
            }
            let port = callback_port(&snapshot.pending)?;
            if !CODEX_OAUTH_CALLBACK_PORTS.contains(&port) {
                self.inner.cleanup(&snapshot.login_id)?;
                continue;
            }
            if align_sign_in_proxy(&mut snapshot, sign_in_proxy_id) {
                write_snapshot(&self.inner.root, &snapshot)?;
            }
            if snapshot.status == OAuthFlowStatus::CallbackReceived {
                return Ok(snapshot.start());
            }
            if lock(&self.inner.listeners).contains_key(&snapshot.login_id) {
                return Ok(snapshot.start());
            }
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                self.spawn_listener(listener, snapshot.clone(), now_ms);
                self.inner
                    .emit(&snapshot.login_id, OAuthFlowStatus::Pending);
                return Ok(snapshot.start());
            }
        }

        let listener = bind_callback_listener().await?;
        let port = listener
            .local_addr()
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::ListenerUnavailable,
                    "OAuth callback listener address is unavailable",
                )
            })?
            .port();
        let login_id = Uuid::new_v4().hyphenated().to_string();
        let start = oauth.begin(port, now_ms).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::ListenerUnavailable,
                "OAuth login could not be initialized",
            )
        })?;
        let snapshot = PendingSnapshot {
            version: SNAPSHOT_VERSION,
            login_id: login_id.clone(),
            authorization_url: start.authorization_url().to_string(),
            callback_secret_ref: callback_secret_ref(&login_id),
            status: OAuthFlowStatus::Pending,
            target_account_id: target_account_id.map(str::to_string),
            sign_in_proxy_id: sign_in_proxy_id.map(str::to_string),
            pending: start.into_pending(),
        };
        write_snapshot(&self.inner.root, &snapshot)?;
        self.spawn_listener(listener, snapshot.clone(), now_ms);
        self.inner.emit(&login_id, OAuthFlowStatus::Pending);
        Ok(snapshot.start())
    }

    pub fn sign_in_proxy_id(&self, login_id: &str) -> Result<Option<String>, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        Ok(read_snapshot(&self.inner.root, &login_id)?.sign_in_proxy_id)
    }

    pub async fn resume(&self, login_id: &str) -> Result<OAuthFlowStart, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        let snapshot = read_snapshot(&self.inner.root, &login_id)?;
        if snapshot.pending.expires_at_ms() <= now_ms() {
            self.inner.cleanup(&login_id)?;
            return Err(
                OAuthFlowError::new(OAuthFlowErrorCode::Expired, "OAuth login expired")
                    .for_login(&login_id),
            );
        }
        let port = callback_port(&snapshot.pending)?;
        if !CODEX_OAUTH_CALLBACK_PORTS.contains(&port) {
            self.inner.cleanup(&login_id)?;
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::Expired,
                "OAuth login must be restarted",
            )
            .for_login(&login_id));
        }
        if snapshot.status == OAuthFlowStatus::Pending
            && !lock(&self.inner.listeners).contains_key(&login_id)
        {
            let listener = TcpListener::bind(("127.0.0.1", port)).await.map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackPortUnavailable,
                    "OAuth callback port is unavailable",
                )
                .for_login(&login_id)
            })?;
            self.spawn_listener(listener, snapshot.clone(), now_ms());
        }
        self.inner.emit(&login_id, snapshot.status);
        Ok(snapshot.start())
    }

    pub fn status(&self, login_id: &str) -> Result<OAuthFlowStart, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        read_snapshot(&self.inner.root, &login_id).map(|snapshot| snapshot.start())
    }

    pub async fn submit_manual_callback(
        &self,
        login_id: &str,
        callback_url: &str,
    ) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.inner.accept_callback(&login_id, callback_url)?;
        self.stop_listener(&login_id).await;
        Ok(())
    }

    pub fn exchange_material(
        &self,
        login_id: &str,
    ) -> Result<OAuthExchangeMaterial, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        let _mutation = lock(&self.inner.mutation);
        let snapshot = read_snapshot(&self.inner.root, &login_id)?;
        if snapshot.status != OAuthFlowStatus::CallbackReceived {
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::SecretMissing,
                "OAuth callback has not been received",
            )
            .for_login(&login_id));
        }
        let callback_url = self
            .inner
            .secrets
            .load(&snapshot.callback_secret_ref)
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretStoreUnavailable,
                    "OAuth callback secret store is unavailable",
                )
                .for_login(&login_id)
            })?
            .ok_or_else(|| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretMissing,
                    "OAuth callback secret is missing",
                )
                .for_login(&login_id)
            })?;
        let callback = snapshot
            .pending
            .parse_callback(&callback_url, now_ms())
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackInvalid,
                    "OAuth callback is invalid",
                )
                .for_login(&login_id)
            })?;
        Ok(OAuthExchangeMaterial {
            pending: snapshot.pending,
            callback,
        })
    }

    pub async fn cancel(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.stop_listener(&login_id).await;
        self.inner.cleanup(&login_id)?;
        self.inner.emit(&login_id, OAuthFlowStatus::Canceled);
        Ok(())
    }

    pub async fn complete(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.stop_listener(&login_id).await;
        self.inner.cleanup(&login_id)?;
        self.inner.emit(&login_id, OAuthFlowStatus::Completed);
        Ok(())
    }

    #[cfg(test)]
    pub async fn shutdown(&self) {
        let controls = {
            let mut listeners = lock(&self.inner.listeners);
            listeners
                .drain()
                .map(|(_, control)| control)
                .collect::<Vec<_>>()
        };
        for mut control in controls {
            if let Some(shutdown) = control.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = control.task.await;
        }
    }

    fn spawn_listener(&self, listener: TcpListener, snapshot: PendingSnapshot, started_at_ms: u64) {
        let login_id = snapshot.login_id.clone();
        let inner = Arc::clone(&self.inner);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (start_tx, start_rx) = oneshot::channel();
        let task_login_id = login_id.clone();
        let task = tokio::spawn(async move {
            if start_rx.await.is_ok() {
                run_listener(
                    Arc::clone(&inner),
                    listener,
                    snapshot,
                    started_at_ms,
                    shutdown_rx,
                )
                .await;
            }
            lock(&inner.listeners).remove(&task_login_id);
        });
        lock(&self.inner.listeners).insert(
            login_id,
            ListenerControl {
                shutdown: Some(shutdown_tx),
                task,
            },
        );
        let _ = start_tx.send(());
    }

    async fn stop_listener(&self, login_id: &str) {
        let control = lock(&self.inner.listeners).remove(login_id);
        if let Some(mut control) = control {
            if let Some(shutdown) = control.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = control.task.await;
        }
    }
}

fn align_sign_in_proxy(snapshot: &mut PendingSnapshot, sign_in_proxy_id: Option<&str>) -> bool {
    let Some(proxy_id) = sign_in_proxy_id else {
        return false;
    };
    if snapshot.status == OAuthFlowStatus::CallbackReceived
        || snapshot.sign_in_proxy_id.as_deref() == Some(proxy_id)
    {
        return false;
    }
    snapshot.sign_in_proxy_id = Some(proxy_id.to_string());
    true
}
