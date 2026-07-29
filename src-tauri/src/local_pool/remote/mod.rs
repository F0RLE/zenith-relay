pub mod client;
pub mod deployment;
pub mod origin;

pub use crate::local_pool::models::RemoteTargetRecord;
use crate::local_pool::{error::Result, store::secret_store};

pub fn load_token(target: &RemoteTargetRecord) -> Result<Option<String>> {
    secret_store::load(&target.secret_ref)
}

pub fn save_token(target: &RemoteTargetRecord, token: &str) -> Result<()> {
    secret_store::save(&target.secret_ref, token)
}

pub fn delete_token(target: &RemoteTargetRecord) -> Result<()> {
    secret_store::delete(&target.secret_ref)
}
