use super::*;
use tokio::sync::oneshot;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

/// Own the socket while idle so peer heartbeats are serviced between turns.
pub(super) struct IdleSocket {
    resume: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<IdleConnection, ConnectionResetReason>>,
}

impl Drop for IdleSocket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl IdleSocket {
    pub(super) fn retain(mut connection: IdleConnection) -> Self {
        let (resume, resumed) = oneshot::channel();
        let task = tokio::spawn(async move {
            let age_remaining = MAX_AGE.saturating_sub(connection.opened.elapsed());
            let lifetime = IDLE_TTL.min(age_remaining);
            let expired = tokio::time::sleep(lifetime);
            tokio::pin!(expired);
            tokio::pin!(resumed);
            let mut heartbeat = tokio::time::interval_at(
                tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
                HEARTBEAT_INTERVAL,
            );
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    resumed = &mut resumed => {
                        if tokio::time::Instant::now() >= expired.deadline() {
                            return Err(if age_remaining <= IDLE_TTL {
                                ConnectionResetReason::ConnectionExpired
                            } else {
                                ConnectionResetReason::SessionExpired
                            });
                        }
                        return resumed.map(|()| connection)
                            .map_err(|_| ConnectionResetReason::ResponseInterrupted);
                    }
                    () = &mut expired => return Err(if age_remaining <= IDLE_TTL {
                        ConnectionResetReason::ConnectionExpired
                    } else {
                        ConnectionResetReason::SessionExpired
                    }),
                    message = connection.socket.next() => match message {
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                            tokio::time::timeout(CONNECT_TIMEOUT, connection.socket.flush())
                                .await.map_err(|_| ConnectionResetReason::HeartbeatFailed)?
                                .map_err(|_| ConnectionResetReason::HeartbeatFailed)?;
                        }
                        Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                            return Err(ConnectionResetReason::PendingFrame);
                        }
                        _ => return Err(ConnectionResetReason::ConnectionClosed),
                    },
                    _ = heartbeat.tick() => {
                        tokio::time::timeout(CONNECT_TIMEOUT,
                            connection.socket.send(Message::Ping(Vec::new().into())))
                            .await.map_err(|_| ConnectionResetReason::HeartbeatFailed)?
                            .map_err(|_| ConnectionResetReason::HeartbeatFailed)?;
                    }
                }
            }
        });
        Self {
            resume: Some(resume),
            task,
        }
    }

    pub(super) async fn resume(mut self) -> Result<IdleConnection, ConnectionResetReason> {
        if let Some(resume) = self.resume.take() {
            let _ = resume.send(());
        }
        (&mut self.task)
            .await
            .unwrap_or(Err(ConnectionResetReason::ConnectionClosed))
    }
}
