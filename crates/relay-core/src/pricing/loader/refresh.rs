use super::super::payload_hash;
use super::{
    now_ms, CatalogRefreshOutcome, CatalogStatus, PricingCacheEnvelope, PricingCatalog,
    PricingCatalogLoader, PricingError, MAX_CATALOG_RESPONSE_BYTES,
};
use crate::catalog_io::{self, CatalogIoError};
use reqwest::{header, StatusCode};

impl PricingCatalogLoader {
    pub async fn refresh(&self, force: bool) -> Result<CatalogRefreshOutcome, PricingError> {
        if !force && !self.refresh_due(now_ms()) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        let _guard = self.refresh_lock.lock().await;
        if !force && !self.refresh_due(now_ms()) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        self.set_status(CatalogStatus::Updating, None);
        let mut request = self.client.get(super::super::LITELLM_SOURCE_URL);
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        if let Some(envelope) = current_envelope.as_ref() {
            if let Some(etag) = envelope.etag.as_deref() {
                request = request.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = envelope.last_modified.as_deref() {
                request = request.header(header::IF_MODIFIED_SINCE, last_modified);
            }
        }
        let (response, permit) = match crate::scheduler::refresh::http::management_http_gate()
            .send(
                &self.client,
                request,
                crate::scheduler::refresh::http::HttpClass::Ordinary,
            )
            .await
        {
            Ok(result) => result,
            Err(_) => return self.refresh_failed(PricingError::Network),
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            let result = self.accept_not_modified(response).await;
            drop(permit);
            return result;
        }
        if response.status() != StatusCode::OK {
            return self.refresh_failed(PricingError::HttpStatus(response.status().as_u16()));
        }
        let response_headers = response.headers().clone();
        let payload = match catalog_io::response_json(response, MAX_CATALOG_RESPONSE_BYTES).await {
            Ok(payload) => payload,
            Err(error) => return self.refresh_failed(map_catalog_io_error(error, false)),
        };
        drop(permit);
        let payload_sha256 = match payload_hash(&payload) {
            Ok(hash) => hash,
            Err(error) => return self.refresh_failed(error),
        };
        let fetched_at_ms = now_ms();
        let mut envelope = match PricingCacheEnvelope::new(payload, payload_sha256, fetched_at_ms) {
            Ok(envelope) => envelope,
            Err(error) => return self.refresh_failed(error),
        };
        envelope.etag = header_string(&response_headers, header::ETAG);
        envelope.last_modified = header_string(&response_headers, header::LAST_MODIFIED);
        envelope.stale = false;
        let catalog = match PricingCatalog::from_litellm_payload(
            &envelope.payload,
            Some(envelope.revision.clone()),
            Some(envelope.fetched_at_ms),
            false,
        ) {
            Ok(catalog) => catalog,
            Err(error) => return self.refresh_failed(error),
        };
        // Keep validators and freshness metadata current even when the
        // payload itself is unchanged. The serialized envelope is still
        // replaced only after complete parsing and validation.
        if let Err(error) = self.store.write_if_changed(&envelope) {
            return self.refresh_failed(error);
        }
        self.handle.replace(catalog);
        *self
            .envelope
            .write()
            .expect("pricing envelope lock poisoned") = Some(envelope.clone());
        self.set_status(CatalogStatus::Current, None);
        self.record_refresh_success();
        Ok(CatalogRefreshOutcome::Updated {
            revision: envelope.revision,
        })
    }

    async fn accept_not_modified(
        &self,
        response: reqwest::Response,
    ) -> Result<CatalogRefreshOutcome, PricingError> {
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        let Some(mut current) = current_envelope else {
            return self.refresh_failed(PricingError::InvalidCache);
        };
        if let Some(etag) = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
        {
            current.etag = Some(etag.to_string());
        }
        if let Some(last_modified) = response
            .headers()
            .get(header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
        {
            current.last_modified = Some(last_modified.to_string());
        }
        current.fetched_at_ms = now_ms();
        current.stale = false;
        let catalog = match current.catalog() {
            Ok(catalog) => catalog,
            Err(error) => return self.refresh_failed(error),
        };
        if let Err(error) = self.store.write_if_changed(&current) {
            return self.refresh_failed(error);
        }
        self.handle.replace(catalog);
        *self
            .envelope
            .write()
            .expect("pricing envelope lock poisoned") = Some(current.clone());
        self.set_status(CatalogStatus::Current, None);
        self.record_refresh_success();
        Ok(CatalogRefreshOutcome::NotModified {
            revision: current.revision,
        })
    }

    pub(super) fn refresh_failed<T>(&self, error: PricingError) -> Result<T, PricingError> {
        self.record_refresh_failure(now_ms());
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        if let Some(envelope) = current_envelope {
            let mut stale = envelope;
            stale.stale = true;
            if let Ok(catalog) = stale.catalog() {
                self.handle.replace(catalog);
            }
            // Persist the marker so a restart does not mistake a catalog that
            // failed its last refresh for a current snapshot. The refresh
            // error remains the primary diagnostic even if this best-effort
            // write also fails.
            let _ = self.store.write_if_changed(&stale);
            *self
                .envelope
                .write()
                .expect("pricing envelope lock poisoned") = Some(stale);
            self.set_status(CatalogStatus::Stale, Some(error));
        } else {
            self.set_status(CatalogStatus::Error, Some(error));
        }
        // Wake the scheduler only after the retry deadline, stale marker, and
        // externally visible status have been updated as one logical result.
        // Otherwise it could observe the previous schedule and immediately
        // go back to sleep with stale state.
        self.schedule_changed.notify_one();
        Err(error)
    }
}

pub(super) fn map_catalog_io_error(error: CatalogIoError, cache: bool) -> PricingError {
    match error {
        CatalogIoError::TooLarge => PricingError::CacheTooLarge,
        CatalogIoError::Io => PricingError::Io,
        CatalogIoError::Network => PricingError::Network,
        CatalogIoError::InvalidJson if cache => PricingError::InvalidCache,
        CatalogIoError::InvalidJson => PricingError::InvalidCatalog,
    }
}

// Kept private so the compiler catches accidental use of a removed response
// path; validators are read directly before body collection in `refresh`.
fn header_string(headers: &reqwest::header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}
