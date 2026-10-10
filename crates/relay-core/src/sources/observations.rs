//! Non-secret metadata retained by read-only provider adapters. Response hints
//! survive parse failures, timeouts and unsuccessful host persistence.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct SourceReadHints {
    retry_after_ms: AtomicU64,
}
impl SourceReadHints {
    pub(crate) fn observe(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(delay) = crate::transport::retry_after_ms(headers, std::time::SystemTime::now())
        {
            self.retry_after_ms.fetch_max(delay, Ordering::Relaxed);
        }
    }
    pub(crate) fn delay(&self) -> Option<u64> {
        let retry_after_delay_ms = self.retry_after_ms.load(Ordering::Relaxed);
        (retry_after_delay_ms > 0).then_some(retry_after_delay_ms)
    }
}

pub struct SourceRead<T> {
    pub read_value: T,
    pub retry_after_ms: Option<u64>,
}
