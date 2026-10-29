use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use tokio::sync::OwnedSemaphorePermit;

#[cfg(test)]
mod tests;

pub(super) async fn run<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    project: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        // A canceled caller cannot stop a running blocking task. Keep its slot
        // until projection actually finishes, rather than spawning a successor.
        let _permit = permit;
        project()
    })
    .await
    .map_err(|_| LocalPoolError::new(ErrorCode::Io, "Runtime snapshot read was interrupted"))?
}
