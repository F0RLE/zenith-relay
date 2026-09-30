use super::refresh::map_catalog_io_error;
use super::{PricingCacheEnvelope, PricingCatalog, PricingError, MAX_CATALOG_RESPONSE_BYTES};
use crate::catalog_io;
use std::path::{Path, PathBuf};

/// Small synchronous persistence boundary shared by the desktop and server
/// loaders. It never parses untrusted data without validating the complete
/// envelope first and replaces the target only after the temporary file is
/// flushed successfully.
#[derive(Clone, Debug)]
pub struct PricingCacheStore {
    path: PathBuf,
}

impl PricingCacheStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Option<PricingCacheEnvelope>, PricingError> {
        let envelope =
            catalog_io::read_json::<PricingCacheEnvelope>(&self.path, MAX_CATALOG_RESPONSE_BYTES)
                .map_err(|error| map_catalog_io_error(error, true))?;
        envelope
            .map(|envelope| {
                envelope.validate()?;
                Ok(envelope)
            })
            .transpose()
    }

    pub fn read_catalog(
        &self,
    ) -> Result<Option<(PricingCacheEnvelope, PricingCatalog)>, PricingError> {
        self.read()?
            .map(|envelope| envelope.catalog().map(|catalog| (envelope, catalog)))
            .transpose()
    }

    pub fn write(&self, envelope: &PricingCacheEnvelope) -> Result<(), PricingError> {
        envelope.validate()?;
        catalog_io::write_json_if_changed(&self.path, envelope, MAX_CATALOG_RESPONSE_BYTES)
            .map(|_| ())
            .map_err(|error| map_catalog_io_error(error, true))
    }

    /// Persist an envelope only when its serialized value changed.  A refresh
    /// can legitimately keep the same payload while changing validators or
    /// freshness metadata, so equality is checked on the complete validated
    /// envelope rather than on the payload hash alone.
    pub fn write_if_changed(&self, envelope: &PricingCacheEnvelope) -> Result<bool, PricingError> {
        envelope.validate()?;
        catalog_io::write_json_if_changed(&self.path, envelope, MAX_CATALOG_RESPONSE_BYTES)
            .map_err(|error| map_catalog_io_error(error, true))
    }
}
